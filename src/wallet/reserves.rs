// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Proof of reserves
//!
//! This module provides the ability to create proofs of reserves.
//! A proof is a valid but unspendable transaction. By signing a transaction
//! that spends some UTXOs we are proofing that we have control over these funds.
//! The implementation is inspired by the following BIPs:
//! https://github.com/bitcoin/bips/blob/master/bip-0127.mediawiki
//! https://github.com/bitcoin/bips/blob/master/bip-0322.mediawiki

use bitcoin::blockdata::opcodes;
use bitcoin::blockdata::script::{Builder, Script};
use bitcoin::blockdata::transaction::{OutPoint, SigHashType, TxIn, TxOut};
use bitcoin::consensus::encode::serialize;
use bitcoin::hash_types::{PubkeyHash, Txid};
use bitcoin::hashes::{hash160, sha256d, Hash};
use bitcoin::util::address::Payload;
use bitcoin::util::psbt::{Input, PartiallySignedTransaction as PSBT};
use bitcoin::Network;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use crate::database::BatchDatabase;
use crate::error::Error;
use crate::wallet::tx_builder::TxOrdering;
use crate::wallet::Wallet;

/// The API for proof of reserves
pub trait ProofOfReserves {
    /// Create a proof for all spendable UTXOs in a wallet
    fn create_proof(&self, message: &str) -> Result<PSBT, Error>;

    /// Make sure this is a proof, and not a spendable transaction.
    /// Make sure the proof is valid.
    /// Currently proofs can only be validated against the tip of the chain.
    /// If some of the UTXOs in the proof were spent in the meantime, the proof will fail.
    /// We can currently not validate whether it was valid at a certain block height.
    /// Returns the spendable amount of the proof.
    fn verify_proof(&self, psbt: &PSBT, message: &str) -> Result<u64, Error>;
}

/// Proof error
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ProofError {
    /// Less than two inputs
    WrongNumberOfInputs,
    /// Must have exactly 1 output
    WrongNumberOfOutputs,
    /// Challenge input does not match
    ChallengeInputMismatch,
    /// Found an input other than the challenge which is not spendable. Holds the position of the input.
    NonSpendableInput(usize),
    /// Found an input that has no signature at position
    NotSignedInput(usize),
    /// Found an input with an unsupported SIGHASH type at position
    UnsupportedSighashType(usize),
    /// Found an input that is neither witness nor legacy at position
    NeitherWitnessNorLegacy(usize),
    /// Signature validation failed
    SignatureValidation(usize, String),
    /// The output is not valid
    InvalidOutput,
    /// Input and output values are not equal, implying a miner fee
    InAndOutValueNotEqual,
    /// No matching outpoing found
    OutpointNotFound(usize),
}

impl<B, D> ProofOfReserves for Wallet<B, D>
where
    D: BatchDatabase,
{
    fn create_proof(&self, message: &str) -> Result<PSBT, Error> {
        let challenge_txin = challenge_txin(message);
        let challenge_psbt_inp = Input {
            witness_utxo: Some(TxOut {
                value: 0,
                script_pubkey: Builder::new().push_opcode(opcodes::OP_TRUE).into_script(),
            }),
            final_script_sig: Some(Script::default()), /* "finalize" the input with an empty scriptSig */
            ..Default::default()
        };

        let pkh = PubkeyHash::from_hash(hash160::Hash::hash(&[0]));
        let out_script_unspendable = bitcoin::Address {
            payload: Payload::PubkeyHash(pkh),
            network: self.network,
        }
        .script_pubkey();

        let mut builder = self.build_tx();
        builder
            .drain_wallet()
            .add_foreign_utxo(challenge_txin.previous_output, challenge_psbt_inp, 42)?
            .fee_absolute(0)
            .only_witness_utxo()
            .set_single_recipient(out_script_unspendable)
            .ordering(TxOrdering::Untouched);
        let (psbt, _details) = builder.finish().unwrap();

        Ok(psbt)
    }

    fn verify_proof(&self, psbt: &PSBT, message: &str) -> Result<u64, Error> {
        // verify the proof UTXOs are still spendable
        let outpoints = self
            .list_unspent()?
            .iter()
            .map(|utxo| (utxo.outpoint, utxo.txout.clone()))
            .collect();

        verify_proof(psbt, message, outpoints, self.network)
    }
}

/// Make sure this is a proof, and not a spendable transaction.
/// Make sure the proof is valid.
/// Currently proofs can only be validated against the tip of the chain.
/// If some of the UTXOs in the proof were spent in the meantime, the proof will fail.
/// We can currently not validate whether it was valid at a certain block height.
/// Returns the spendable amount of the proof.
pub fn verify_proof(
    psbt: &PSBT,
    message: &str,
    outpoints: Vec<(OutPoint, TxOut)>,
    network: Network,
) -> Result<u64, Error> {
    let tx = psbt.clone().extract_tx();

    if tx.output.len() != 1 {
        return Err(Error::Proof(ProofError::WrongNumberOfOutputs));
    }
    if tx.input.len() <= 1 {
        return Err(Error::Proof(ProofError::WrongNumberOfInputs));
    }

    // verify the challenge txin
    let challenge_txin = challenge_txin(message);
    if tx.input[0].previous_output != challenge_txin.previous_output {
        return Err(Error::Proof(ProofError::ChallengeInputMismatch));
    }

    // verify the proof UTXOs are still spendable
    if let Some((i, _inp)) = tx
        .input
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_i, inp)| outpoints.iter().find(|op| op.0 == inp.previous_output) == None)
    {
        return Err(Error::Proof(ProofError::NonSpendableInput(i)));
    }

    // verify that the inputs are signed, except the challenge
    if let Some((i, _inp)) = psbt
        .inputs
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_i, inp)| inp.final_script_sig.is_none() && inp.final_script_witness.is_none())
    {
        return Err(Error::Proof(ProofError::NotSignedInput(i)));
    }

    // Verify the SIGHASH
    if let Some((i, _inp)) = tx
        .input
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_i, inp)| !verify_sighash_type_all(inp))
    {
        return Err(Error::Proof(ProofError::UnsupportedSighashType(i)));
    }

    let serialized_tx = serialize(&tx);
    // Verify the challenge input
    if let Some(utxo) = &psbt.inputs[0].witness_utxo {
        if let Err(err) = bitcoinconsensus::verify(
            utxo.script_pubkey.to_bytes().as_slice(),
            utxo.value,
            &serialized_tx,
            0,
        ) {
            return Err(Error::Proof(ProofError::SignatureValidation(
                0,
                format!("{:?}", err),
            )));
        }
    } else {
        return Err(Error::Proof(ProofError::SignatureValidation(
            0,
            "witness_utxo not found for challenge input".to_string(),
        )));
    }
    // Verify other inputs against prevouts.
    if let Some((i, res)) = tx
        .input
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, tx_in)| {
            if let Some(op) = outpoints.iter().find(|op| op.0 == tx_in.previous_output) {
                (i, Ok(op.1.clone()))
            } else {
                (i, Err(Error::Proof(ProofError::OutpointNotFound(i))))
            }
        })
        .map(|(i, res)| match res {
            Ok(txout) => (
                i,
                Ok(bitcoinconsensus::verify(
                    txout.script_pubkey.to_bytes().as_slice(),
                    txout.value,
                    &serialized_tx,
                    i,
                )),
            ),
            Err(err) => (i, Err(err)),
        })
        .find(|(_i, res)| res.is_err())
    {
        return Err(Error::Proof(ProofError::SignatureValidation(
            i,
            format!("{:?}", res.err().unwrap()),
        )));
    }

    // calculate the spendable amount of the proof
    let sum = tx
        .input
        .iter()
        .map(|tx_in| {
            if let Some(op) = outpoints.iter().find(|op| op.0 == tx_in.previous_output) {
                op.1.value
            } else {
                0
            }
        })
        .sum();

    // verify the unspendable output
    let pkh = PubkeyHash::from_hash(hash160::Hash::hash(&[0]));
    let out_script_unspendable = bitcoin::Address {
        payload: Payload::PubkeyHash(pkh),
        network,
    }
    .script_pubkey();
    if tx.output[0].script_pubkey != out_script_unspendable {
        return Err(Error::Proof(ProofError::InvalidOutput));
    }

    // inflow and outflow being equal means no miner fee
    if tx.output[0].value != sum {
        return Err(Error::Proof(ProofError::InAndOutValueNotEqual));
    }

    Ok(sum)
}

/// Construct a challenge input with the message
fn challenge_txin(message: &str) -> TxIn {
    let message = "Proof-of-Reserves: ".to_string() + message;
    let message = sha256d::Hash::hash(message.as_bytes());
    TxIn {
        previous_output: OutPoint::new(Txid::from_hash(message), 0),
        sequence: 0xFFFFFFFF,
        ..Default::default()
    }
}

/// Verify the SIGHASH type for a TxIn
fn verify_sighash_type_all(inp: &TxIn) -> bool {
    if inp.witness.is_empty() {
        if let Some(sht_int) = inp.script_sig.as_bytes().last() {
            #[allow(clippy::if_same_then_else)]
            #[allow(clippy::needless_bool)]
            if *sht_int == 174 {
                // ToDo: What is the meaning of this?
                true
            } else if let Ok(sht) = SigHashType::from_u32_standard(*sht_int as u32) {
                if sht == SigHashType::All {
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        for wit in &inp.witness[..inp.witness.len() - 1] {
            // ToDo: Why do we skip the last element?
            if wit.last().is_none() {
                // ToDo: Why are there empty elements?
                continue;
            }
            if let Ok(sht) = SigHashType::from_u32_standard(*wit.last().unwrap() as u32) {
                if SigHashType::All != sht {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod test {
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::util::key::{PrivateKey, PublicKey};
    use bitcoin::Network;
    use rstest::rstest;

    use super::*;
    use crate::blockchain::{noop_progress, ElectrumBlockchain};
    use crate::database::memory::MemoryDatabase;
    use crate::electrum_client::Client;
    use crate::wallet::test::get_funded_wallet;
    use crate::SignOptions;

    #[rstest(
        descriptor,
        case("wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)"),
        case("wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))"),     // and(pk(Alice),older(6))
        case("wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100000)))") // and(pk(Alice),after(100000))
    )]
    fn test_proof(descriptor: &'static str) -> Result<(), Error> {
        let (wallet, _, _) = get_funded_wallet(descriptor);
        let balance = wallet.get_balance()?;

        let message = "This belongs to me.";
        let mut psbt = wallet.create_proof(&message)?;
        let num_inp = psbt.inputs.len();
        assert!(
            num_inp > 1,
            "num_inp is {} but should be more than 1",
            num_inp
        );

        let finalized = wallet.sign(
            &mut psbt,
            SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )?;
        let num_sigs = psbt
            .inputs
            .iter()
            .fold(0, |acc, i| acc + i.partial_sigs.len());
        assert_eq!(num_sigs, num_inp - 1);
        assert!(finalized);

        let spendable = wallet.verify_proof(&psbt, &message)?;
        assert_eq!(spendable, balance);

        // additional temporary checks
        match descriptor {
            "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)" => {
                let tx = psbt.extract_tx();
                assert_eq!(tx.input.len(), 2);
                let txin = &tx.input[1];
                assert_eq!(txin.witness.len(), 2);
                assert_eq!(txin.script_sig.len(), 0);
                assert_eq!(txin.witness[0].len(), 71);
                assert_eq!(txin.witness[1].len(), 33);
                assert_eq!(txin.witness[0], vec![48, 68, 2, 32, 38, 53, 34, 73, 249, 21, 56, 117, 41, 128, 169, 39, 62, 213, 31, 44, 155, 35, 92, 72, 115, 7, 109, 71, 51, 146, 206, 98, 39, 232, 190, 209, 2, 32, 79, 16, 232, 251, 225, 31, 117, 45, 146, 104, 183, 113, 208, 209, 6, 209, 219, 58, 137, 157, 116, 6, 242, 34, 255, 179, 182, 18, 6, 148, 142, 127, 1]);
                assert_eq!(txin.witness[1], vec![3, 43, 5, 88, 7, 139, 236, 56, 105, 74, 132, 147, 61, 101, 147, 3, 226, 87, 93, 174, 126, 145, 104, 89, 17, 69, 65, 21, 191, 214, 68, 135, 227]);
            }
            "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))" => {
                let tx = psbt.extract_tx();
                assert_eq!(tx.input.len(), 2);
                let txin = &tx.input[1];
                assert_eq!(txin.witness.len(), 2);
                assert_eq!(txin.script_sig.len(), 0);
                assert_eq!(txin.witness[0].len(), 71);
                assert_eq!(txin.witness[1].len(), 37);
                assert_eq!(txin.witness[0], vec![48, 68, 2, 32, 9, 75, 75, 249, 73, 139, 223, 112, 98, 163, 248, 70, 132, 28, 43, 36, 80, 193, 49, 199, 11, 175, 177, 233, 24, 18, 157, 120, 240, 22, 40, 184, 2, 32, 121, 245, 45, 179, 155, 59, 126, 164, 59, 94, 229, 236, 251, 222, 176, 100, 76, 115, 167, 158, 80, 165, 40, 2, 9, 177, 208, 72, 98, 188, 192, 84, 1]);
                assert_eq!(txin.witness[1], vec![33, 3, 43, 5, 88, 7, 139, 236, 56, 105, 74, 132, 147, 61, 101, 147, 3, 226, 87, 93, 174, 126, 145, 104, 89, 17, 69, 65, 21, 191, 214, 68, 135, 227, 173, 86, 178]);
            }
            "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100000)))" => {
                let tx = psbt.extract_tx();
                assert_eq!(tx.input.len(), 2);
                let txin = &tx.input[1];
                assert_eq!(txin.witness.len(), 2);
                assert_eq!(txin.script_sig.len(), 0);
                assert_eq!(txin.witness[0].len(), 72);
                assert_eq!(txin.witness[1].len(), 40);
                assert_eq!(txin.witness[0], vec![48, 69, 2, 33, 0, 129, 112, 186, 87, 10, 201, 60, 176, 239, 226, 217, 254, 222, 11, 72, 220, 154, 120, 89, 193, 177, 57, 133, 41, 11, 81, 233, 37, 15, 229, 167, 11, 2, 32, 80, 74, 124, 217, 113, 88, 92, 181, 217, 40, 236, 30, 251, 66, 160, 218, 39, 248, 153, 99, 17, 43, 23, 229, 39, 140, 28, 66, 47, 18, 238, 53, 1]);
                assert_eq!(txin.witness[1], vec![33, 3, 43, 5, 88, 7, 139, 236, 56, 105, 74, 132, 147, 61, 101, 147, 3, 226, 87, 93, 174, 126, 145, 104, 89, 17, 69, 65, 21, 191, 214, 68, 135, 227, 173, 3, 160, 134, 1, 177]);
            }
            _ => panic!("should not happen"),
        }

        Ok(())
    }

    #[test]
    #[should_panic(expected = "Proof(ChallengeInputMismatch)")]
    fn tampered_proof_message() {
        let descriptor = "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
        let (wallet, _, _) = get_funded_wallet(descriptor);
        let balance = wallet.get_balance().unwrap();

        let message_alice = "This belongs to Alice.";
        let mut psbt_alice = wallet.create_proof(&message_alice).unwrap();

        let signopt = SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };
        let _finalized = wallet.sign(&mut psbt_alice, signopt).unwrap();

        let spendable = wallet.verify_proof(&psbt_alice, &message_alice).unwrap();
        assert_eq!(spendable, balance);

        // change the message
        let message_bob = "This belongs to Bob.";
        let psbt_bob = wallet.create_proof(&message_bob).unwrap();
        psbt_alice.global.unsigned_tx.input[0].previous_output =
            psbt_bob.global.unsigned_tx.input[0].previous_output;
        psbt_alice.inputs[0].witness_utxo = psbt_bob.inputs[0].witness_utxo.clone();

        let res_alice = wallet.verify_proof(&psbt_alice, &message_alice);
        let res_bob = wallet.verify_proof(&psbt_alice, &message_bob);
        assert!(res_alice.is_err());
        assert!(!res_bob.is_err());
        res_alice.unwrap();
        res_bob.unwrap();
    }

    #[test]
    #[should_panic(expected = "Proof(UnsupportedSighashType(1)")]
    fn tampered_proof_sighash_tx() {
        let descriptor = "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
        let (wallet, _, _) = get_funded_wallet(descriptor);

        let message = "This belongs to Alice.";
        let mut psbt = wallet.create_proof(&message).unwrap();

        let signopt = SignOptions {
            trust_witness_utxo: true,
            allow_all_sighashes: true,
            ..Default::default()
        };

        // set an unsupported sighash
        psbt.inputs[1].sighash_type = Some(SigHashType::Single);

        let _finalized = wallet.sign(&mut psbt, signopt).unwrap();

        let _spendable = wallet.verify_proof(&psbt, &message).unwrap();
    }

    #[test]
    #[should_panic(expected = "Proof(InAndOutValueNotEqual)")]
    fn tampered_proof_miner_fee() {
        let descriptor = "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
        let (wallet, _, _) = get_funded_wallet(descriptor);

        let message = "This belongs to Alice.";
        let mut psbt = wallet.create_proof(&message).unwrap();

        let signopt = SignOptions {
            trust_witness_utxo: true,
            allow_all_sighashes: true,
            ..Default::default()
        };

        // reduce the output value to grant a miner fee
        psbt.global.unsigned_tx.output[0].value -= 100;

        let _finalized = wallet.sign(&mut psbt, signopt).unwrap();

        let _spendable = wallet.verify_proof(&psbt, &message).unwrap();
    }

    enum MultisigType {
        Wsh,
        ShWsh,
        P2sh,
    }

    fn construct_multisig_wallet(
        signer: &PrivateKey,
        pubkeys: &[PublicKey],
        script_type: &MultisigType,
    ) -> Result<Wallet<ElectrumBlockchain, MemoryDatabase>, Error> {
        let secp = Secp256k1::new();
        let pub_derived = signer.public_key(&secp);

        let (prefix, postfix) = match script_type {
            MultisigType::Wsh => ("wsh(", ")"),
            MultisigType::ShWsh => ("sh(wsh(", "))"),
            MultisigType::P2sh => ("sh(", ")"),
        };
        let prefix = prefix.to_string() + "multi(2,";
        let postfix = postfix.to_string() + ")";
        let desc = pubkeys.iter().enumerate().fold(prefix, |acc, (i, pubkey)| {
            let mut desc = acc;
            if i != 0 {
                desc += ",";
            }
            if *pubkey == pub_derived {
                desc += &signer.to_wif();
            } else {
                desc += &pubkey.to_string();
            }
            desc
        }) + &postfix;

        let client = Client::new("ssl://electrum.blockstream.info:60002")?;
        let wallet = Wallet::new(
            &desc,
            None,
            Network::Testnet,
            MemoryDatabase::default(),
            ElectrumBlockchain::from(client),
        )?;

        wallet.sync(noop_progress(), None)?;

        Ok(wallet)
    }

    #[rstest(
        script_type,
        expected_address,
        case(
            MultisigType::Wsh,
            "tb1qnmhmxkaqqz4lrruhew5mk6zqr0ezstn3stj6c3r2my6hgkescm0sg3qc0r"
        ),
        case(MultisigType::ShWsh, "2NDTiUegP4NwKMnxXm6KdCL1B1WHamhZHC1"),
        case(MultisigType::P2sh, "2N7yrzYXgQzNQQuHNTjcP3iwpzFVsqe6non")
    )]
    fn test_proof_multisig(
        script_type: MultisigType,
        expected_address: &'static str,
    ) -> Result<(), Error> {
        let signer1 =
            PrivateKey::from_wif("cQCi6JdidZN5HeiHhjE7zZAJ1XJrZbj6MmpVPx8Ri3Kc8UjPgfbn").unwrap();
        let signer2 =
            PrivateKey::from_wif("cTTgG6x13nQjAeECaCaDrjrUdcjReZBGspcmNavsnSRyXq7zXT7r").unwrap();
        let signer3 =
            PrivateKey::from_wif("cUPkz3JBZinD1RRU7ngmx8cssqJ4KgBvboq1QZcGfyjqm8L6etRH").unwrap();
        let secp = Secp256k1::new();
        let mut pubkeys = vec![
            signer1.public_key(&secp),
            signer2.public_key(&secp),
            signer3.public_key(&secp),
        ];
        pubkeys.sort_by_key(|item| item.to_string());

        let wallet1 = construct_multisig_wallet(&signer1, &pubkeys, &script_type)?;
        let wallet2 = construct_multisig_wallet(&signer2, &pubkeys, &script_type)?;
        let wallet3 = construct_multisig_wallet(&signer3, &pubkeys, &script_type)?;
        assert_eq!(wallet1.get_new_address()?.to_string(), expected_address);
        assert_eq!(wallet2.get_new_address()?.to_string(), expected_address);
        assert_eq!(wallet3.get_new_address()?.to_string(), expected_address);
        let balance = wallet1.get_balance()?;
        assert!(
            (410000..=420000).contains(&balance),
            "balance is {} but should be between 410000 and 420000",
            balance
        );

        let message = "All my precious coins";
        let mut psbt = wallet1.create_proof(message)?;
        let num_inp = psbt.inputs.len();
        assert!(
            num_inp > 1,
            "num_inp is {} but should be more than 1",
            num_inp
        );

        // returns a tuple with the counts of (partial_sigs, final_script_sig, final_script_witness)
        let count_signatures = |psbt: &PSBT| {
            psbt.inputs.iter().fold((0usize, 0, 0), |acc, i| {
                (
                    acc.0 + i.partial_sigs.len(),
                    acc.1 + if i.final_script_sig.is_some() { 1 } else { 0 },
                    acc.2
                        + if i.final_script_witness.is_some() {
                            1
                        } else {
                            0
                        },
                )
            })
        };

        let signopts = SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };
        let finalized = wallet1.sign(&mut psbt, signopts.clone())?;
        assert_eq!(count_signatures(&psbt), (num_inp - 1, 1, 0));
        assert!(!finalized);

        let finalized = wallet2.sign(&mut psbt, signopts.clone())?;
        assert_eq!(
            count_signatures(&psbt),
            ((num_inp - 1) * 2, num_inp, num_inp - 1)
        );
        assert!(finalized);

        // 2 signatures are enough. Just checking what happens...
        let finalized = wallet3.sign(&mut psbt, signopts.clone())?;
        assert_eq!(
            count_signatures(&psbt),
            ((num_inp - 1) * 2, num_inp, num_inp - 1)
        );
        assert!(finalized);

        let finalized = wallet1.finalize_psbt(&mut psbt, signopts)?;
        assert_eq!(
            count_signatures(&psbt),
            ((num_inp - 1) * 2, num_inp, num_inp - 1)
        );
        assert!(finalized);

        // additional temporary checks
        match script_type {
            MultisigType::Wsh => {
                let tx = psbt.clone().extract_tx();
                assert_eq!(tx.input.len(), 2);
                let txin = &tx.input[1];
                assert_eq!(txin.witness.len(), 4);
                assert_eq!(txin.script_sig.len(), 0);
                assert_eq!(txin.witness[0].len(), 0);
                assert_eq!(txin.witness[1].len(), 72);
                assert_eq!(txin.witness[2].len(), 72);
                assert_eq!(txin.witness[3].len(), 105);
                assert_eq!(
                    txin.witness[1],
                    vec![
                        48, 69, 2, 33, 0, 171, 133, 140, 57, 207, 82, 96, 69, 210, 155, 200, 52,
                        115, 36, 240, 220, 145, 81, 89, 24, 31, 18, 45, 4, 195, 231, 246, 242, 13,
                        23, 2, 100, 2, 32, 21, 23, 134, 76, 123, 229, 9, 211, 37, 181, 73, 20, 193,
                        74, 93, 137, 164, 227, 104, 118, 154, 54, 3, 211, 151, 209, 203, 31, 139,
                        148, 203, 106, 1
                    ]
                );
                assert_eq!(
                    txin.witness[2],
                    vec![
                        48, 69, 2, 33, 0, 130, 9, 233, 255, 114, 175, 169, 62, 234, 225, 138, 107,
                        35, 134, 82, 77, 131, 189, 240, 164, 49, 213, 53, 111, 51, 79, 91, 51, 10,
                        204, 4, 180, 2, 32, 88, 21, 142, 187, 197, 167, 24, 105, 77, 116, 189, 136,
                        5, 18, 202, 145, 19, 139, 75, 180, 185, 46, 129, 129, 201, 225, 123, 5,
                        182, 47, 148, 185, 1
                    ]
                );
                assert_eq!(
                    txin.witness[3],
                    vec![
                        82, 33, 2, 5, 128, 246, 213, 193, 72, 37, 223, 179, 53, 9, 132, 24, 26,
                        213, 12, 163, 129, 213, 184, 112, 60, 166, 28, 248, 235, 104, 189, 63, 95,
                        172, 172, 33, 2, 166, 123, 134, 89, 178, 94, 143, 195, 164, 240, 85, 28,
                        187, 155, 22, 120, 8, 1, 253, 207, 106, 91, 21, 54, 121, 28, 251, 37, 85,
                        221, 56, 231, 33, 3, 121, 25, 56, 158, 67, 99, 99, 210, 140, 34, 214, 49,
                        87, 84, 248, 9, 19, 4, 237, 255, 35, 98, 175, 72, 67, 232, 58, 170, 234,
                        28, 195, 131, 83, 174
                    ]
                );
            }
            MultisigType::ShWsh => {
                let tx = psbt.clone().extract_tx();
                assert_eq!(tx.input.len(), 2);
                let txin = &tx.input[1];
                assert_eq!(txin.witness.len(), 4);
                assert_eq!(txin.script_sig.len(), 35);
                assert_eq!(txin.witness[0].len(), 0);
                assert_eq!(txin.witness[1].len(), 71);
                assert_eq!(txin.witness[2].len(), 71);
                assert_eq!(txin.witness[3].len(), 105);
                assert_eq!(
                    txin.witness[1],
                    vec![
                        48, 68, 2, 32, 115, 178, 228, 102, 102, 178, 180, 26, 84, 63, 216, 60, 247,
                        251, 114, 236, 96, 119, 54, 228, 5, 236, 26, 199, 189, 105, 70, 241, 208,
                        133, 153, 189, 2, 32, 126, 101, 10, 179, 132, 249, 156, 159, 169, 194, 53,
                        34, 52, 85, 97, 29, 23, 35, 238, 18, 170, 130, 10, 184, 157, 104, 55, 115,
                        133, 14, 92, 78, 1
                    ]
                );
                assert_eq!(
                    txin.witness[2],
                    vec![
                        48, 68, 2, 32, 38, 84, 239, 143, 248, 86, 171, 110, 32, 124, 133, 252, 183,
                        143, 158, 75, 133, 10, 59, 129, 250, 60, 17, 10, 179, 192, 19, 145, 3, 62,
                        203, 197, 2, 32, 110, 198, 250, 19, 50, 37, 208, 57, 57, 133, 82, 211, 64,
                        120, 250, 33, 123, 248, 68, 16, 251, 113, 162, 119, 194, 241, 242, 130,
                        195, 38, 40, 239, 1
                    ]
                );
                assert_eq!(
                    txin.witness[3],
                    vec![
                        82, 33, 2, 5, 128, 246, 213, 193, 72, 37, 223, 179, 53, 9, 132, 24, 26,
                        213, 12, 163, 129, 213, 184, 112, 60, 166, 28, 248, 235, 104, 189, 63, 95,
                        172, 172, 33, 2, 166, 123, 134, 89, 178, 94, 143, 195, 164, 240, 85, 28,
                        187, 155, 22, 120, 8, 1, 253, 207, 106, 91, 21, 54, 121, 28, 251, 37, 85,
                        221, 56, 231, 33, 3, 121, 25, 56, 158, 67, 99, 99, 210, 140, 34, 214, 49,
                        87, 84, 248, 9, 19, 4, 237, 255, 35, 98, 175, 72, 67, 232, 58, 170, 234,
                        28, 195, 131, 83, 174
                    ]
                );
            }
            MultisigType::P2sh => {
                let tx = psbt.clone().extract_tx();
                assert_eq!(tx.input.len(), 4);
                let txin = &tx.input[1];
                assert_eq!(txin.witness.len(), 0);
                assert_eq!(txin.script_sig.len(), 252);
                assert_eq!(
                    txin.script_sig.as_bytes(),
                    vec![
                        0, 71, 48, 68, 2, 32, 97, 26, 132, 211, 161, 8, 118, 150, 201, 249, 97,
                        205, 144, 34, 67, 71, 47, 204, 225, 151, 249, 68, 212, 32, 213, 118, 61,
                        60, 180, 235, 82, 108, 2, 32, 119, 242, 247, 13, 18, 164, 36, 200, 144,
                        166, 165, 153, 39, 201, 151, 237, 14, 96, 49, 183, 146, 236, 151, 245, 9,
                        13, 171, 109, 16, 59, 37, 114, 1, 71, 48, 68, 2, 32, 30, 111, 238, 231,
                        141, 160, 199, 182, 158, 23, 100, 176, 99, 113, 23, 121, 27, 61, 84, 125,
                        221, 181, 101, 131, 155, 2, 110, 74, 236, 146, 23, 43, 2, 32, 72, 225, 67,
                        21, 114, 13, 125, 224, 52, 50, 93, 27, 134, 201, 102, 116, 133, 67, 100,
                        101, 137, 4, 137, 117, 124, 167, 101, 134, 198, 29, 138, 63, 1, 76, 105,
                        82, 33, 2, 5, 128, 246, 213, 193, 72, 37, 223, 179, 53, 9, 132, 24, 26,
                        213, 12, 163, 129, 213, 184, 112, 60, 166, 28, 248, 235, 104, 189, 63, 95,
                        172, 172, 33, 2, 166, 123, 134, 89, 178, 94, 143, 195, 164, 240, 85, 28,
                        187, 155, 22, 120, 8, 1, 253, 207, 106, 91, 21, 54, 121, 28, 251, 37, 85,
                        221, 56, 231, 33, 3, 121, 25, 56, 158, 67, 99, 99, 210, 140, 34, 214, 49,
                        87, 84, 248, 9, 19, 4, 237, 255, 35, 98, 175, 72, 67, 232, 58, 170, 234,
                        28, 195, 131, 83, 174
                    ]
                );
            }
        }

        let spendable = wallet1.verify_proof(&psbt, &message)?;
        assert_eq!(spendable, balance);

        Ok(())
    }
}
