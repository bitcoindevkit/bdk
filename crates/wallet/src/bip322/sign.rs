//! The signature generation implementation for BIP-322 for message signing
//! according to the BIP-322 standard.

use alloc::{string::String, vec::Vec};
use bitcoin::{
    absolute::LockTime,
    base64::{prelude::BASE64_STANDARD, Engine},
    consensus::encode::serialize,
    key::{Keypair, TapTweak},
    psbt::Input,
    secp256k1::{ecdsa::Signature, Message},
    sighash::{self, SighashCache},
    sign_message::signed_msg_hash,
    transaction::Version,
    Address, Amount, EcdsaSighashType, OutPoint, PrivateKey, Psbt, PublicKey, ScriptBuf, Sequence,
    TapSighashType, Transaction, TxIn, TxOut, Witness,
};
use core::str::FromStr;
use std::string::ToString;

use crate::utils::SecpCtx;

use super::{
    utils::{to_sign, to_spend},
    BIP322Error, BIP322Signature, Bip322SignatureFormat,
};

impl BIP322Signature {
    /// Creates a new instance of `BIP322Signature` with the specified parameters.
    ///
    /// # Arguments
    /// - `private_key_str`: A WIF-encoded private key as a `String`.
    /// - `message`: The message to be signed.
    /// - `address`: The Bitcoin address for which the signature is intended.
    /// - `signature_type`: The BIP322 signature format to be used. Can be one of Legacy, Simple, or Full.
    /// - `proof_of_funds`: An optional vector of UTXO information tuples, where each tuple contains:
    ///    - `OutPoint`: A reference to a previous transaction output.
    ///    - `ScriptBuf`: The script for the UTXO.
    ///    - `Witness`: The witness data associated with the UTXO.
    ///    - `Sequence`: The sequence number.
    ///
    /// # Returns
    /// An instance of `BIP322Signature`.
    ///
    /// # Example
    /// ```
    /// # use bdk_wallet::bip322::{BIP322Signature, Bip322SignatureFormat};
    ///
    /// let signer = BIP322Signature::new(
    ///     "c...".to_string(),
    ///     "Hello, Bitcoin!".to_string(),
    ///     "1BitcoinAddress...".to_string(),
    ///     Bip322SignatureFormat::Legacy,
    ///     None,
    /// );
    /// ```
    pub fn new(
        private_key_str: String,
        message: String,
        address_str: String,
        signature_type: Bip322SignatureFormat,
        proof_of_funds: Option<Vec<(OutPoint, ScriptBuf, Witness, Sequence)>>,
    ) -> Self {
        Self {
            private_key_str,
            message,
            address_str,
            signature_type,
            proof_of_funds,
        }
    }

    /// Signs a message using a provided private key, message, and address with a specified
    /// BIP322 format (Legacy, Simple, or Full).
    ///
    /// - **Legacy:** Generates a traditional ECDSA signature for P2PKH addresses.
    /// - **Simple:** Constructs a simplified signature by signing the message and encoding
    ///   the witness data.
    /// - **Full:** Creates a comprehensive signature by building and signing an entire
    ///   transaction, including all witness details.
    ///
    /// The function extracts the necessary key and script information from the input,
    /// processes any optional proof of funds, and returns the resulting signature as a
    /// Base64-encoded string.
    ///
    /// # Errors
    /// Returns a `BIP322Error` if any signing steps fail.
    pub fn sign(&self) -> Result<String, BIP322Error> {
        let secp = SecpCtx::new();
        let private_key = PrivateKey::from_wif(&self.private_key_str)
            .map_err(|_| BIP322Error::InvalidPrivateKey)?;
        let pubkey = private_key.public_key(&secp);

        let script_pubkey = Address::from_str(&self.address_str)
            .map_err(|_| BIP322Error::InvalidAddress)?
            .assume_checked()
            .script_pubkey();

        match &self.signature_type {
            Bip322SignatureFormat::Legacy => {
                if !script_pubkey.is_p2pkh() {
                    return Err(BIP322Error::InvalidFormat("legacy".to_string()));
                }

                let sig_serialized = self.sign_legacy(&private_key)?;
                Ok(BASE64_STANDARD.encode(sig_serialized))
            }
            Bip322SignatureFormat::Simple => {
                let witness = self.sign_message(&private_key, pubkey, &script_pubkey)?;

                Ok(BASE64_STANDARD.encode(serialize(&witness.input[0].witness.clone())))
            }
            Bip322SignatureFormat::Full => {
                let transaction = self.sign_message(&private_key, pubkey, &script_pubkey)?;

                Ok(BASE64_STANDARD.encode(serialize(&transaction)))
            }
        }
    }

    /// Constructs a transaction that includes a signature for the provided message
    /// according to the BIP322 message signing protocol.
    ///
    /// This function builds the transaction to be signed by selecting the appropriate
    /// signing method based on the script type:
    /// - P2WPKH
    /// - P2TR
    /// - P2SH
    ///
    /// Optionally, if a proof of funds is provided, additional inputs are appended
    /// to support advanced verification scenarios. On success, the function returns a
    /// complete transaction
    ///
    /// # Errors
    /// Returns a `BIP322Error` if the script type is unsupported or if any part of
    /// the signing process fails.
    fn sign_message(
        &self,
        private_key: &PrivateKey,
        pubkey: PublicKey,
        script_pubkey: &ScriptBuf,
    ) -> Result<Transaction, BIP322Error> {
        let to_spend = to_spend(script_pubkey, &self.message);
        let mut to_sign = to_sign(
            &to_spend.output[0].script_pubkey,
            to_spend.compute_txid(),
            to_spend.lock_time,
            to_spend.input[0].sequence,
            Some(to_spend.input[0].witness.clone()),
        )?;

        if let Some(proofs) = self.proof_of_funds.clone() {
            for (previous_output, script_sig, witness, sequence) in proofs {
                to_sign.inputs.push(Input {
                    non_witness_utxo: Some(Transaction {
                        input: vec![TxIn {
                            previous_output,
                            script_sig,
                            sequence,
                            witness,
                        }],
                        output: vec![],
                        version: Version(2),
                        lock_time: LockTime::ZERO,
                    }),
                    ..Default::default()
                })
            }
        }

        let mut sighash_cache = SighashCache::new(&to_sign.unsigned_tx);

        let witness = if script_pubkey.is_p2wpkh() {
            self.sign_p2sh_p2wpkh(&mut sighash_cache, to_spend, private_key, pubkey, true)?
        } else if script_pubkey.is_p2tr() || script_pubkey.is_p2wsh() {
            self.sign_p2tr(&mut sighash_cache, to_spend, to_sign.clone(), private_key)?
        } else if script_pubkey.is_p2sh() {
            self.sign_p2sh_p2wpkh(&mut sighash_cache, to_spend, private_key, pubkey, false)?
        } else {
            return Err(BIP322Error::UnsupportedType);
        };

        to_sign.inputs[0].final_script_witness = Some(witness);

        let transaction = to_sign
            .extract_tx()
            .map_err(|_| BIP322Error::ExtractionError("transaction".to_string()))?;

        Ok(transaction)
    }

    fn sign_legacy(&self, private_key: &PrivateKey) -> Result<Vec<u8>, BIP322Error> {
        let secp = SecpCtx::new();

        let message_hash = signed_msg_hash(&self.message);
        let message = &Message::from_digest_slice(message_hash.as_ref())
            .map_err(|_| BIP322Error::InvalidMessage)?;

        let mut signature: Signature = secp.sign_ecdsa(message, &private_key.inner);
        signature.normalize_s();
        let mut sig_serialized = signature.serialize_der().to_vec();
        sig_serialized.push(EcdsaSighashType::All as u8);

        Ok(sig_serialized)
    }

    fn sign_p2sh_p2wpkh(
        &self,
        sighash_cache: &mut SighashCache<&Transaction>,
        to_spend: Transaction,
        private_key: &PrivateKey,
        pubkey: PublicKey,
        is_segwit: bool,
    ) -> Result<Witness, BIP322Error> {
        let secp = SecpCtx::new();
        let sighash_type = EcdsaSighashType::All;

        let wpubkey_hash = &pubkey
            .wpubkey_hash()
            .map_err(|e| BIP322Error::InvalidPublicKey(e.to_string()))?;

        let sighash = sighash_cache
            .p2wpkh_signature_hash(
                0,
                &if is_segwit {
                    to_spend.output[0].script_pubkey.clone()
                } else {
                    ScriptBuf::new_p2wpkh(wpubkey_hash)
                },
                to_spend.output[0].value,
                sighash_type,
            )
            .map_err(|_| BIP322Error::SighashError)?;

        let msg = &Message::from_digest_slice(sighash.as_ref())
            .map_err(|_| BIP322Error::InvalidMessage)?;

        let signature = secp.sign_ecdsa(msg, &private_key.inner);
        let mut sig_serialized = signature.serialize_der().to_vec();
        sig_serialized.push(sighash_type as u8);

        Ok(Witness::from(vec![
            sig_serialized,
            pubkey.inner.serialize().to_vec(),
        ]))
    }

    fn sign_p2tr(
        &self,
        sighash_cache: &mut SighashCache<&Transaction>,
        to_spend: Transaction,
        mut to_sign: Psbt,
        private_key: &PrivateKey,
    ) -> Result<Witness, BIP322Error> {
        let secp = SecpCtx::new();
        let keypair = Keypair::from_secret_key(&secp, &private_key.inner);
        let key_pair = keypair
            .tap_tweak(&secp, to_sign.inputs[0].tap_merkle_root)
            .to_inner();
        let x_only_public_key = keypair.x_only_public_key().0;

        let sighash_type = TapSighashType::All;

        to_sign.inputs[0].tap_internal_key = Some(x_only_public_key);

        let sighash = sighash_cache
            .taproot_key_spend_signature_hash(
                0,
                &sighash::Prevouts::All(&[TxOut {
                    value: Amount::from_sat(0),
                    script_pubkey: to_spend.output[0].clone().script_pubkey,
                }]),
                sighash_type,
            )
            .map_err(|_| BIP322Error::SighashError)?;

        let msg = &Message::from_digest_slice(sighash.as_ref())
            .map_err(|_| BIP322Error::InvalidMessage)?;

        let signature = secp.sign_schnorr_no_aux_rand(msg, &key_pair);
        let mut sig_serialized = signature.serialize().to_vec();
        sig_serialized.push(sighash_type as u8);

        Ok(Witness::from(vec![sig_serialized]))
    }
}
