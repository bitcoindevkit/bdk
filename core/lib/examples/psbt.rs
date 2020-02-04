extern crate base64;
extern crate magical_bitcoin_wallet;

use std::str::FromStr;

use magical_bitcoin_wallet::bitcoin;
use magical_bitcoin_wallet::descriptor::*;
use magical_bitcoin_wallet::psbt::*;
use magical_bitcoin_wallet::signer::Signer;

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::util::psbt::PartiallySignedTransaction;
use bitcoin::SigHashType;

fn main() {
    let desc = "pkh(tprv8ZgxMBicQKsPd7Uf69XL1XwhmjHopUGep8GuEiJDZmbQz6o58LninorQAfcKZWARbtRtfnLcJ5MQ2AtHcQJCCRUcMRvmDUjyEmNUWwx8UbK/*)";

    let extended_desc = ExtendedDescriptor::from_str(desc).unwrap();

    let psbt_str = "cHNidP8BAFMCAAAAAd9SiQfxXZ+CKjgjRNonWXsnlA84aLvjxtwCmMfRc0ZbAQAAAAD+////ASjS9QUAAAAAF6kUYJR3oB0lS1M0W1RRMMiENSX45IuHAAAAAAABAPUCAAAAA9I7/OqeFeOFdr5VTLnj3UI/CNRw2eWmMPf7qDv6uIF6AAAAABcWABTG+kgr0g44V0sK9/9FN9oG/CxMK/7///+d0ffphPcV6FE9J/3ZPKWu17YxBnWWTJQyRJs3HUo1gwEAAAAA/v///835mYd9DmnjVnUKd2421MDoZmIxvB4XyJluN3SPUV9hAAAAABcWABRfvwFGp+x/yWdXeNgFs9v0duyeS/7///8CFbH+AAAAAAAXqRSEnTOAjJN/X6ZgR9ftKmwisNSZx4cA4fUFAAAAABl2qRTs6pS4x17MSQ4yNs/1GPsfdlv2NIisAAAAACIGApVE9PPtkcqp8Da43yrXGv4nLOotZdyxwJoTWQxuLxIuCAxfmh4JAAAAAAA=";
    let psbt_buf = base64::decode(psbt_str).unwrap();
    let mut psbt: PartiallySignedTransaction = deserialize(&psbt_buf).unwrap();

    let signer = PSBTSigner::from_descriptor(&psbt.global.unsigned_tx, &extended_desc).unwrap();

    for (index, input) in psbt.inputs.iter_mut().enumerate() {
        for (pubkey, (fing, path)) in &input.hd_keypaths {
            let sighash = input.sighash_type.unwrap_or(SigHashType::All);

            // Ignore the "witness_utxo" case because we know this psbt is a legacy tx
            if let Some(non_wit_utxo) = &input.non_witness_utxo {
                let prev_script = &non_wit_utxo.output
                    [psbt.global.unsigned_tx.input[index].previous_output.vout as usize]
                    .script_pubkey;
                let (signature, sighash) = signer
                    .sig_legacy_from_fingerprint(index, sighash, fing, path, prev_script)
                    .unwrap()
                    .unwrap();

                let mut concat_sig = Vec::new();
                concat_sig.extend_from_slice(&signature.serialize_der());
                concat_sig.extend_from_slice(&[sighash as u8]);

                input.partial_sigs.insert(*pubkey, concat_sig);
            }
        }
    }

    println!("signed: {}", base64::encode(&serialize(&psbt)));
}
