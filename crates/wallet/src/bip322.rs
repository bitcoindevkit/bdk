//! The BIP-322 module provides functionality for message signing
//! according to the BIP-322 standard.

use alloc::{string::String, vec::Vec};
use bitcoin::{
    absolute::LockTime,
    base64::{prelude::BASE64_STANDARD, Engine},
    blockdata::opcodes::all::OP_RETURN,
    consensus::{encode::serialize, Decodable},
    hashes::{sha256, Hash, HashEngine},
    io::Cursor,
    key::{Keypair, TapTweak},
    opcodes::OP_0,
    psbt::Input,
    script::Builder,
    secp256k1::{ecdsa::Signature, schnorr, Message},
    sighash::{self, SighashCache},
    sign_message::signed_msg_hash,
    transaction::Version,
    Address, Amount, EcdsaSighashType, OutPoint, PrivateKey, Psbt, PublicKey, ScriptBuf, Sequence,
    TapSighashType, Transaction, TxIn, TxOut, Txid, Witness, WitnessVersion, XOnlyPublicKey,
};
use core::{fmt, str::FromStr};
use std::string::ToString;

use crate::utils::SecpCtx;

/// Represents the different formats supported by the BIP322 message signing protocol.
///
/// BIP322 defines multiple formats for signatures to accommodate different use cases
/// and maintain backward compatibility with legacy signing methods.
#[derive(Debug, PartialEq)]
pub enum Bip322SignatureFormat {
    /// The legacy Bitcoin message signature format used before BIP322.
    Legacy,
    /// A simplified version of the BIP322 format that includes only essential data.
    Simple,
    /// The Full BIP322 format that includes all signature data.
    Full,
}

/// Error types for BIP322 message signing and verification operations.
///
/// This enum encompasses all possible errors that can occur during the BIP322
/// message signing or verification process.
#[derive(Debug)]
pub enum BIP322Error {
    /// Error encountered when extracting data, such as from a PSBT
    ExtractionError(String),
    /// Unable to compute the signature hash for signing
    SighashError,
    /// The message does not meet requirements
    InvalidMessage,
    /// The script or address type is not supported
    UnsupportedType,
    /// The format of the data is invalid for the given context
    InvalidFormat(String),
    /// The provided private key is invalid
    InvalidPrivateKey,
    /// The provided public key is invalid
    InvalidPublicKey(String),
    /// The provided Bitcoin address is invalid
    InvalidAddress,
    /// The address is not a Segwit address
    NotSegwitAddress,
    /// The Segwit version is not supported for the given context
    UnsupportedSegwitVersion(String),
    /// The digital signature is invalid
    InvalidSignature(String),
    /// The transaction witness data is invalid
    InvalidWitness(String),
    /// Error encountered when decoding Bitcoin consensus data
    DecodeError(String),
    /// Error encountered when decoding Base64 data
    Base64DecodeError,
    /// The provided sighash type is invalid for this context
    InvalidSighashType,
}

impl std::error::Error for BIP322Error {}

impl std::fmt::Display for BIP322Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BIP322Error::ExtractionError(e) => write!(f, "Unable to extract {}", e),
            BIP322Error::SighashError => write!(f, "Unable to compute signature hash"),
            BIP322Error::InvalidMessage => write!(f, "Message hash is not secure"),
            BIP322Error::UnsupportedType => write!(f, "Type is not supported"),
            BIP322Error::InvalidFormat(e) => write!(f, "Only valid for {} format", e),
            BIP322Error::InvalidPrivateKey => write!(f, "Invalid private key"),
            BIP322Error::InvalidPublicKey(e) => write!(f, "Invalid public key {}", e),
            BIP322Error::InvalidAddress => write!(f, "Invalid address"),
            BIP322Error::NotSegwitAddress => write!(f, "Not a Segwit address"),
            BIP322Error::UnsupportedSegwitVersion(e) => write!(f, "Only Segwit {} is supported", e),
            BIP322Error::InvalidSignature(e) => write!(f, "Invalid Signature - {}", e),
            BIP322Error::InvalidWitness(e) => write!(f, "Invalid Witness - {}", e),
            BIP322Error::InvalidSighashType => write!(f, "Sighash type is invalid"),
            BIP322Error::DecodeError(e) => write!(f, "Consensus decode error - {}", e),
            BIP322Error::Base64DecodeError => write!(f, "Base64 decoding failed"),
        }
    }
}

/// Creates a tagged hash of a message according to the BIP322 specification.
pub fn tagged_message_hash(message: &[u8]) -> sha256::Hash {
    let tag = "BIP0322-signed-message";
    let mut engine = sha256::Hash::engine();

    let tag_hash = sha256::Hash::hash(tag.as_bytes());
    engine.input(&tag_hash[..]);
    engine.input(&tag_hash[..]);
    engine.input(message);

    sha256::Hash::from_engine(engine)
}

/// Constructs the "to_spend" transaction according to the BIP322 specification.
pub fn to_spend(script_pubkey: &ScriptBuf, message: &str) -> Transaction {
    let txid = Txid::from_slice(&[0u8; 32]).expect("Txid slice error");

    let outpoint = OutPoint {
        txid,
        vout: 0xFFFFFFFF,
    };
    let message_hash = tagged_message_hash(message.as_bytes());
    let script_sig = Builder::new()
        .push_opcode(OP_0)
        .push_slice(message_hash.to_byte_array())
        .into_script();

    Transaction {
        version: Version(0),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig,
            sequence: Sequence::ZERO,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(0),
            script_pubkey: script_pubkey.clone(),
        }],
    }
}

/// Constructs a transaction according to the BIP322 specification.
///
/// This transaction will be signed to prove ownership of the private key
/// corresponding to the script_pubkey.
///
/// Returns a PSBT (Partially Signed Bitcoin Transaction) ready for signing
/// or a [`BIP322Error`] if something goes wrong.
pub fn to_sign(
    script_pubkey: &ScriptBuf,
    txid: Txid,
    lock_time: LockTime,
    sequence: Sequence,
    witness: Option<Witness>,
) -> Result<Psbt, BIP322Error> {
    let outpoint = OutPoint { txid, vout: 0x00 };
    let script_pub_key = Builder::new().push_opcode(OP_RETURN).into_script();

    let tx = Transaction {
        version: Version(0),
        lock_time,
        input: vec![TxIn {
            previous_output: outpoint,
            sequence,
            script_sig: ScriptBuf::new(),
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(0),
            script_pubkey: script_pub_key,
        }],
    };

    let mut psbt =
        Psbt::from_unsigned_tx(tx).map_err(|_| BIP322Error::ExtractionError("psbt".to_string()))?;

    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(0),
        script_pubkey: script_pubkey.clone(),
    });

    psbt.inputs[0].final_script_witness = witness;

    Ok(psbt)
}

fn sign_legacy(message: &str, private_key: PrivateKey) -> Result<Vec<u8>, BIP322Error> {
    let secp = SecpCtx::new();

    let message_hash = signed_msg_hash(message);
    let message = &Message::from_digest_slice(message_hash.as_ref())
        .map_err(|_| BIP322Error::InvalidMessage)?;

    let mut signature: Signature = secp.sign_ecdsa(message, &private_key.inner);
    signature.normalize_s();
    let mut sig_serialized = signature.serialize_der().to_vec();
    sig_serialized.push(EcdsaSighashType::All as u8);

    Ok(sig_serialized)
}

fn sign_p2sh_p2wpkh(
    sighash_cache: &mut SighashCache<&Transaction>,
    to_spend: Transaction,
    private_key: PrivateKey,
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

    let msg =
        &Message::from_digest_slice(sighash.as_ref()).map_err(|_| BIP322Error::InvalidMessage)?;

    let signature = secp.sign_ecdsa(msg, &private_key.inner);
    let mut sig_serialized = signature.serialize_der().to_vec();
    sig_serialized.push(sighash_type as u8);

    Ok(Witness::from(vec![
        sig_serialized,
        pubkey.inner.serialize().to_vec(),
    ]))
}

fn sign_p2tr(
    sighash_cache: &mut SighashCache<&Transaction>,
    to_spend: Transaction,
    mut to_sign: Psbt,
    private_key: PrivateKey,
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

    let msg =
        &Message::from_digest_slice(sighash.as_ref()).map_err(|_| BIP322Error::InvalidMessage)?;

    let signature = secp.sign_schnorr_no_aux_rand(msg, &key_pair);
    let mut sig_serialized = signature.serialize().to_vec();
    sig_serialized.push(sighash_type as u8);

    Ok(Witness::from(vec![sig_serialized]))
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
pub fn sign_message(
    private_key: PrivateKey,
    pubkey: PublicKey,
    script_pubkey: &ScriptBuf,
    message: &str,
    proof_of_funds: &Option<Vec<(OutPoint, ScriptBuf, Witness, Sequence)>>,
) -> Result<Transaction, BIP322Error> {
    let to_spend = to_spend(script_pubkey, message);
    let mut to_sign = to_sign(
        &to_spend.output[0].script_pubkey,
        to_spend.compute_txid(),
        to_spend.lock_time,
        to_spend.input[0].sequence,
        Some(to_spend.input[0].witness.clone()),
    )?;

    if let Some(proofs) = proof_of_funds {
        for (prevout, script, witness, sequence) in proofs {
            to_sign.inputs.push(Input {
                non_witness_utxo: Some(Transaction {
                    input: vec![TxIn {
                        previous_output: *prevout,
                        script_sig: script.clone(),
                        sequence: *sequence,
                        witness: witness.clone(),
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
        sign_p2sh_p2wpkh(&mut sighash_cache, to_spend, private_key, pubkey, true)?
    } else if script_pubkey.is_p2tr() || script_pubkey.is_p2wsh() {
        sign_p2tr(&mut sighash_cache, to_spend, to_sign.clone(), private_key)?
    } else if script_pubkey.is_p2sh() {
        sign_p2sh_p2wpkh(&mut sighash_cache, to_spend, private_key, pubkey, false)?
    } else {
        return Err(BIP322Error::UnsupportedType);
    };

    to_sign.inputs[0].final_script_witness = Some(witness);

    let transaction = to_sign
        .extract_tx()
        .map_err(|_| BIP322Error::ExtractionError("transaction".to_string()))?;

    Ok(transaction)
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
pub fn sign(
    priv_key: &str,
    message: &str,
    address: &str,
    signature_type: Bip322SignatureFormat,
    proof_of_funds: &Option<Vec<(OutPoint, ScriptBuf, Witness, Sequence)>>,
) -> Result<String, BIP322Error> {
    let secp = SecpCtx::new();
    let private_key = PrivateKey::from_wif(priv_key).map_err(|_| BIP322Error::InvalidPrivateKey)?;
    let pubkey = private_key.public_key(&secp);

    let script_pubkey = Address::from_str(address)
        .map_err(|_| BIP322Error::InvalidAddress)?
        .assume_checked()
        .script_pubkey();

    match signature_type {
        Bip322SignatureFormat::Legacy => {
            if !script_pubkey.is_p2pkh() {
                return Err(BIP322Error::InvalidFormat("legacy".to_string()));
            }

            let sig_serialized = sign_legacy(message, private_key)?;
            Ok(BASE64_STANDARD.encode(sig_serialized))
        }
        Bip322SignatureFormat::Simple => {
            let witness =
                sign_message(private_key, pubkey, &script_pubkey, message, proof_of_funds)?;

            Ok(BASE64_STANDARD.encode(serialize(&witness.input[0].witness.clone())))
        }
        Bip322SignatureFormat::Full => {
            let transaction =
                sign_message(private_key, pubkey, &script_pubkey, message, proof_of_funds)?;

            Ok(BASE64_STANDARD.encode(serialize(&transaction)))
        }
    }
}

fn verify_legacy(
    signature_bytes: &[u8],
    private_key: PrivateKey,
    message: &str,
) -> Result<bool, BIP322Error> {
    let secp = SecpCtx::new();

    let sig_without_sighash = &signature_bytes[..signature_bytes.len() - 1];

    let pub_key = PublicKey::from_private_key(&secp, &private_key);

    let message_hash = signed_msg_hash(message);
    let msg = &Message::from_digest_slice(message_hash.as_ref())
        .map_err(|_| BIP322Error::InvalidMessage)?;

    let sig = Signature::from_der(sig_without_sighash)
        .map_err(|e| BIP322Error::InvalidSignature(e.to_string()))?;

    let verify = secp.verify_ecdsa(msg, &sig, &pub_key.inner).is_ok();
    Ok(verify)
}

fn verify_p2sh_p2wpkh(
    address: Address,
    to_sign_witness: Transaction,
    to_spend: Transaction,
    to_sign: Psbt,
    is_segwit: bool,
) -> Result<bool, BIP322Error> {
    let secp = SecpCtx::new();
    let pub_key = PublicKey::from_slice(&to_sign_witness.input[0].witness[1])
        .map_err(|e| BIP322Error::InvalidPublicKey(e.to_string()))?;

    if is_segwit {
        let wp = address
            .witness_program()
            .ok_or(BIP322Error::NotSegwitAddress)?;

        if wp.version() != WitnessVersion::V0 {
            return Err(BIP322Error::UnsupportedSegwitVersion("v0".to_string()));
        }
    }

    let to_spend_outpoint = OutPoint {
        txid: to_spend.compute_txid(),
        vout: 0,
    };

    if to_spend_outpoint != to_sign.unsigned_tx.input[0].previous_output {
        return Err(BIP322Error::InvalidSignature(
            "to_sign must spend to_spend output".to_string(),
        ));
    }

    if to_sign.unsigned_tx.output[0].script_pubkey != ScriptBuf::from_bytes(vec![OP_RETURN.to_u8()])
    {
        return Err(BIP322Error::InvalidSignature(
            "to_sign output must be OP_RETURN".to_string(),
        ));
    }

    let witness = to_sign.inputs[0]
        .final_script_witness
        .clone()
        .ok_or(BIP322Error::InvalidWitness("missing data".to_string()))?;

    let encoded_signature = &witness.to_vec()[0];
    let witness_pub_key = &witness.to_vec()[1];
    let signature_length = encoded_signature.len();

    if witness.len() != 2 {
        return Err(BIP322Error::InvalidWitness(
            "invalid witness length".to_string(),
        ));
    }

    if pub_key.to_bytes() != *witness_pub_key {
        return Err(BIP322Error::InvalidPublicKey(
            "public key mismatch".to_string(),
        ));
    }

    let signature = Signature::from_der(&encoded_signature.as_slice()[..signature_length - 1])
        .map_err(|e| BIP322Error::InvalidSignature(e.to_string()))?;
    let sighash_type =
        EcdsaSighashType::from_consensus(encoded_signature[signature_length - 1] as u32);

    if !(sighash_type == EcdsaSighashType::All) {
        return Err(BIP322Error::InvalidSighashType);
    }

    let mut sighash_cache = SighashCache::new(to_sign.unsigned_tx);
    let wpubkey_hash = &pub_key
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

    let msg =
        &Message::from_digest_slice(sighash.as_ref()).map_err(|_| BIP322Error::InvalidMessage)?;

    Ok(secp.verify_ecdsa(msg, &signature, &pub_key.inner).is_ok())
}

fn verify_p2tr(
    address: Address,
    to_spend: Transaction,
    to_sign: Psbt,
    to_sign_witness: Transaction,
) -> Result<bool, BIP322Error> {
    let secp = SecpCtx::new();
    let script_pubkey = address.script_pubkey();
    let witness_program = script_pubkey.as_bytes();

    let pubkey_bytes = &witness_program[2..];

    let pub_key = XOnlyPublicKey::from_slice(pubkey_bytes)
        .map_err(|e| BIP322Error::InvalidPublicKey(e.to_string()))?;

    let wp = address
        .witness_program()
        .ok_or(BIP322Error::NotSegwitAddress)?;

    if wp.version() != WitnessVersion::V1 {
        return Err(BIP322Error::UnsupportedSegwitVersion("v1".to_string()));
    }

    let to_spend_outpoint = OutPoint {
        txid: to_spend.compute_txid(),
        vout: 0,
    };

    if to_spend_outpoint != to_sign.unsigned_tx.input[0].previous_output {
        return Err(BIP322Error::InvalidSignature(
            "to_sign must spend to_spend output".to_string(),
        ));
    }

    if to_sign_witness.output[0].script_pubkey != ScriptBuf::from_bytes(vec![OP_RETURN.to_u8()]) {
        return Err(BIP322Error::InvalidSignature(
            "to_sign output must be OP_RETURN".to_string(),
        ));
    }

    let witness = to_sign.inputs[0]
        .final_script_witness
        .clone()
        .ok_or(BIP322Error::InvalidWitness("missing data".to_string()))?;

    let encoded_signature = &witness.to_vec()[0];
    if witness.len() != 1 {
        return Err(BIP322Error::InvalidWitness(
            "invalid witness length".to_string(),
        ));
    }

    let signature = schnorr::Signature::from_slice(&encoded_signature.as_slice()[..64])
        .map_err(|e| BIP322Error::InvalidSignature(e.to_string()))?;
    let sighash_type = TapSighashType::from_consensus_u8(encoded_signature[64])
        .map_err(|_| BIP322Error::InvalidSighashType)?;

    if sighash_type != TapSighashType::All {
        return Err(BIP322Error::InvalidSighashType);
    }

    let mut sighash_cache = SighashCache::new(to_sign.unsigned_tx);

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

    let msg =
        &Message::from_digest_slice(sighash.as_ref()).map_err(|_| BIP322Error::InvalidMessage)?;

    Ok(secp.verify_schnorr(&signature, msg, &pub_key).is_ok())
}

/// Verifies a BIP322-signed message by reconstructing the underlying transaction data
/// and checking the signature against the provided address and message.
///
/// This function performs the following steps:
/// 1. Constructs a corresponding signing transaction (`to_sign`) using the witness data
///    from the given transaction.
/// 2. It delegates the verification process to the appropriate helper function:
///    - P2WPKH
///    - P2TR
///    - P2SH
/// 3. If none of the supported script types match, the function returns `Ok(false)`.
///
/// # Returns
/// A `Result` containing:
/// - `Ok(true)` if the signature is valid.
/// - `Ok(false)` if the signature does not match the expected verification criteria.
/// - An error of type `BIP322Error` if the verification process fails at any step,
///   such as during transaction reconstruction or when decoding the witness data.
pub fn verify_message(
    address: Address,
    transaction: Transaction,
    message: &str,
) -> Result<bool, BIP322Error> {
    let script_pubkey = address.script_pubkey();

    let to_spend = to_spend(&script_pubkey, message);
    let to_sign = to_sign(
        &to_spend.output[0].script_pubkey,
        to_spend.compute_txid(),
        to_spend.lock_time,
        to_spend.input[0].sequence,
        Some(transaction.input[0].witness.clone()),
    )?;

    if script_pubkey.is_p2wpkh() {
        let verify = verify_p2sh_p2wpkh(address, transaction, to_spend, to_sign, true)?;

        return Ok(verify);
    } else if script_pubkey.is_p2tr() || script_pubkey.is_p2wsh() {
        let verify = verify_p2tr(address, to_spend, to_sign, transaction)?;

        return Ok(verify);
    } else if script_pubkey.is_p2sh() {
        let verify = verify_p2sh_p2wpkh(address, transaction, to_spend, to_sign, false)?;

        return Ok(verify);
    }

    Ok(false)
}

/// Verifies a BIP322 message signature against the provided address and message.
///
/// The verification logic differs depending on the signature format:
/// - Legacy
/// - Simple
/// - Full
///
/// Returns `true` if the signature is valid, or an error if the decoding or verification
/// process fails.
pub fn verify(
    address_str: &str,
    signature: &str,
    message: &str,
    signature_type: Bip322SignatureFormat,
    priv_key: Option<&str>,
) -> Result<bool, BIP322Error> {
    let address = Address::from_str(address_str)
        .map_err(|_| BIP322Error::InvalidAddress)?
        .assume_checked();

    let script_pubkey = address.script_pubkey();

    let signature_bytes = BASE64_STANDARD
        .decode(signature)
        .map_err(|_| BIP322Error::Base64DecodeError)?;

    match signature_type {
        Bip322SignatureFormat::Legacy => {
            let pk = priv_key.ok_or(BIP322Error::InvalidPrivateKey)?;
            let private_key =
                PrivateKey::from_wif(pk).map_err(|_| BIP322Error::InvalidPrivateKey)?;
            verify_legacy(&signature_bytes, private_key, message)
        }
        Bip322SignatureFormat::Simple => {
            let mut cursor = Cursor::new(signature_bytes);
            let witness = Witness::consensus_decode_from_finite_reader(&mut cursor)
                .map_err(|_| BIP322Error::DecodeError("witness".to_string()))?;

            let to_spend_witness = to_spend(&script_pubkey, message);
            let to_sign_witness = to_sign(
                &to_spend_witness.output[0].script_pubkey,
                to_spend_witness.compute_txid(),
                to_spend_witness.lock_time,
                to_spend_witness.input[0].sequence,
                Some(witness),
            )
            .map_err(|_| BIP322Error::ExtractionError("psbt".to_string()))?
            .extract_tx()
            .map_err(|_| BIP322Error::ExtractionError("transaction".to_string()))?;

            verify_message(address, to_sign_witness, message)
        }
        Bip322SignatureFormat::Full => {
            let mut cursor = Cursor::new(signature_bytes);
            let transaction = Transaction::consensus_decode_from_finite_reader(&mut cursor)
                .map_err(|_| BIP322Error::DecodeError("transaction".to_string()))?;

            verify_message(address, transaction, message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Address;
    use core::str::FromStr;
    use std::string::ToString;

    // TEST VECTORS FROM
    // https://github.com/bitcoin/bips/blob/master/bip-0322.mediawiki#user-content-Test_vectors
    // https://github.com/Peach2Peach/bip322-js/tree/main/test

    const PRIVATE_KEY: &str = "L3VFeEujGtevx9w18HD1fhRbCH67Az2dpCymeRE1SoPK6XQtaN2k";
    const PRIVATE_KEY_TESTNET: &str = "cTrF79uahxMC7bQGWh2931vepWPWqS8KtF8EkqgWwv3KMGZNJ2yP";

    const SEGWIT_ADDRESS: &str = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
    const SEGWIT_TESTNET_ADDRESS: &str = "tb1q9vza2e8x573nczrlzms0wvx3gsqjx7vaxwd45v";
    const TAPROOT_ADDRESS: &str = "bc1ppv609nr0vr25u07u95waq5lucwfm6tde4nydujnu8npg4q75mr5sxq8lt3";
    const TAPROOT_TESTNET_ADDRESS: &str =
        "tb1ppv609nr0vr25u07u95waq5lucwfm6tde4nydujnu8npg4q75mr5s3g3s37";
    const LEGACY_ADDRESS: &str = "14vV3aCHBeStb5bkenkNHbe2YAFinYdXgc";
    const LEGACY_ADDRESS_TESTNET: &str = "mjSSLdHFzft9NC5NNMik7WrMQ9rRhMhNpT";

    const HELLO_WORLD_MESSAGE: &str = "Hello World";

    const NESTED_SEGWIT_PRIVATE_KEY: &str = "KwTbAxmBXjoZM3bzbXixEr9nxLhyYSM4vp2swet58i19bw9sqk5z";
    const NESTED_SEGWIT_TESTNET_PRIVATE_KEY: &str =
        "cMpadsm2xoVpWV5FywY5cAeraa1PCtSkzrBM45Ladpf9rgDu6cMz";
    const NESTED_SEGWIT_ADDRESS: &str = "3HSVzEhCFuH9Z3wvoWTexy7BMVVp3PjS6f";
    const NESTED_SEGWIT_TESTNET_ADDRESS: &str = "2N8zi3ydDsMnVkqaUUe5Xav6SZqhyqEduap";

    #[test]
    fn test_message_hashing() {
        let empty_hash = tagged_message_hash(b"");
        let hello_world_hash = tagged_message_hash(b"Hello World");

        assert_eq!(
            empty_hash.to_string(),
            "c90c269c4f8fcbe6880f72a721ddfbf1914268a794cbb21cfafee13770ae19f1"
        );
        assert_eq!(
            hello_world_hash.to_string(),
            "f0eb03b1a75ac6d9847f55c624a99169b5dccba2a31f5b23bea77ba270de0a7a"
        );
    }

    #[test]
    fn test_to_spend_and_to_sign() {
        let script_pubkey = Address::from_str(SEGWIT_ADDRESS)
            .unwrap()
            .assume_checked()
            .script_pubkey();

        // Test case for empty message - to_spend
        let tx_spend_empty_msg = to_spend(&script_pubkey, "");
        assert_eq!(
            tx_spend_empty_msg.compute_txid().to_string(),
            "c5680aa69bb8d860bf82d4e9cd3504b55dde018de765a91bb566283c545a99a7"
        );

        // Test case for "Hello World" - to_spend
        let tx_spend_hello_world_msg = to_spend(&script_pubkey, HELLO_WORLD_MESSAGE);
        assert_eq!(
            tx_spend_hello_world_msg.compute_txid().to_string(),
            "b79d196740ad5217771c1098fc4a4b51e0535c32236c71f1ea4d61a2d603352b"
        );

        // Test case for empty message - to_sign
        let tx_sign_empty_msg = to_sign(
            &tx_spend_empty_msg.output[0].script_pubkey,
            tx_spend_empty_msg.compute_txid(),
            tx_spend_empty_msg.lock_time,
            tx_spend_empty_msg.input[0].sequence,
            Some(tx_spend_empty_msg.input[0].witness.clone()),
        )
        .unwrap();
        assert_eq!(
            tx_sign_empty_msg.unsigned_tx.compute_txid().to_string(),
            "1e9654e951a5ba44c8604c4de6c67fd78a27e81dcadcfe1edf638ba3aaebaed6"
        );

        // Test case for HELLO_WORLD_MESSAGE - to_sign
        let tx_sign_hw_msg = to_sign(
            &tx_spend_hello_world_msg.output[0].script_pubkey,
            tx_spend_hello_world_msg.compute_txid(),
            tx_spend_hello_world_msg.lock_time,
            tx_spend_hello_world_msg.input[0].sequence,
            Some(tx_spend_hello_world_msg.input[0].witness.clone()),
        )
        .unwrap();

        assert_eq!(
            tx_sign_hw_msg.unsigned_tx.compute_txid().to_string(),
            "88737ae86f2077145f93cc4b153ae9a1cb8d56afa511988c149c5c8c9d93bddf"
        );
    }

    #[test]
    fn sign_and_verify_legacy_signature() {
        let sign_message = sign(
            PRIVATE_KEY,
            HELLO_WORLD_MESSAGE,
            LEGACY_ADDRESS,
            Bip322SignatureFormat::Legacy,
            &None,
        )
        .unwrap();

        let sign_message_testnet = sign(
            PRIVATE_KEY_TESTNET,
            HELLO_WORLD_MESSAGE,
            LEGACY_ADDRESS_TESTNET,
            Bip322SignatureFormat::Legacy,
            &None,
        )
        .unwrap();

        let verify_message = verify(
            LEGACY_ADDRESS,
            &sign_message,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Legacy,
            Some(&PRIVATE_KEY),
        )
        .unwrap();

        let verify_message_testnet = verify(
            LEGACY_ADDRESS,
            &sign_message_testnet,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Legacy,
            Some(&PRIVATE_KEY_TESTNET),
        )
        .unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn sign_and_verify_legacy_signature_with_wrong_message() {
        let sign_message = sign(
            PRIVATE_KEY,
            HELLO_WORLD_MESSAGE,
            LEGACY_ADDRESS,
            Bip322SignatureFormat::Legacy,
            &None,
        )
        .unwrap();

        let verify_message = verify(
            LEGACY_ADDRESS,
            &sign_message,
            "",
            Bip322SignatureFormat::Legacy,
            Some(&PRIVATE_KEY),
        )
        .unwrap();

        assert_eq!(verify_message, false);
    }

    #[test]
    fn test_sign_and_verify_nested_segwit_address() {
        let sign_message = sign(
            NESTED_SEGWIT_PRIVATE_KEY,
            HELLO_WORLD_MESSAGE,
            NESTED_SEGWIT_ADDRESS,
            Bip322SignatureFormat::Simple,
            &None,
        )
        .unwrap();
        assert_eq!(sign_message, "AkgwRQIhAMd2wZSY3x0V9Kr/NClochoTXcgDaGl3OObOR17yx3QQAiBVWxqNSS+CKen7bmJTG6YfJjsggQ4Fa2RHKgBKrdQQ+gEhAxa5UDdQCHSQHfKQv14ybcYm1C9y6b12xAuukWzSnS+w");

        let sign_message_testnet = sign(
            NESTED_SEGWIT_TESTNET_PRIVATE_KEY,
            HELLO_WORLD_MESSAGE,
            NESTED_SEGWIT_TESTNET_ADDRESS,
            Bip322SignatureFormat::Full,
            &None,
        )
        .unwrap();
        assert_eq!(sign_message_testnet, "AAAAAAABAVuR8vsJiiYj9+vO+8l7Ol3wt3Frz7SVyVSxn0ehOUb+AAAAAAAAAAAAAQAAAAAAAAAAAWoCSDBFAiEAx3bBlJjfHRX0qv80KWhyGhNdyANoaXc45s5HXvLHdBACIFVbGo1JL4Ip6ftuYlMbph8mOyCBDgVrZEcqAEqt1BD6ASEDFrlQN1AIdJAd8pC/XjJtxibUL3LpvXbEC66RbNKdL7AAAAAA");

        let verify_message = verify(
            NESTED_SEGWIT_ADDRESS,
            &sign_message,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Simple,
            None,
        )
        .unwrap();

        let verify_message_testnet = verify(
            NESTED_SEGWIT_TESTNET_ADDRESS,
            &sign_message_testnet,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Full,
            None,
        )
        .unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn test_sign_and_verify_segwit_address() {
        let sign_message = sign(
            PRIVATE_KEY,
            HELLO_WORLD_MESSAGE,
            SEGWIT_ADDRESS,
            Bip322SignatureFormat::Full,
            &None,
        )
        .unwrap();

        let sign_message_testnet = sign(
            PRIVATE_KEY_TESTNET,
            HELLO_WORLD_MESSAGE,
            SEGWIT_TESTNET_ADDRESS,
            Bip322SignatureFormat::Simple,
            &None,
        )
        .unwrap();

        let verify_message = verify(
            SEGWIT_ADDRESS,
            &sign_message,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Full,
            None,
        )
        .unwrap();

        let verify_message_testnet = verify(
            SEGWIT_TESTNET_ADDRESS,
            &sign_message_testnet,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Simple,
            None,
        )
        .unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn test_sign_and_verify_taproot_address() {
        let sign_message = sign(
            PRIVATE_KEY,
            HELLO_WORLD_MESSAGE,
            TAPROOT_ADDRESS,
            Bip322SignatureFormat::Full,
            &None,
        )
        .unwrap();

        let sign_message_testnet = sign(
            PRIVATE_KEY_TESTNET,
            HELLO_WORLD_MESSAGE,
            TAPROOT_TESTNET_ADDRESS,
            Bip322SignatureFormat::Simple,
            &None,
        )
        .unwrap();

        let verify_message = verify(
            TAPROOT_ADDRESS,
            &sign_message,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Full,
            None,
        )
        .unwrap();

        let verify_message_testnet = verify(
            TAPROOT_TESTNET_ADDRESS,
            &sign_message_testnet,
            HELLO_WORLD_MESSAGE,
            Bip322SignatureFormat::Simple,
            None,
        )
        .unwrap();

        assert!(verify_message);
        assert!(verify_message_testnet);
    }

    #[test]
    fn test_simple_segwit_verification() {
        assert!(
            verify(
                SEGWIT_ADDRESS,
                "AkcwRAIgZRfIY3p7/DoVTty6YZbWS71bc5Vct9p9Fia83eRmw2QCICK/ENGfwLtptFluMGs2KsqoNSk89pO7F29zJLUx9a/sASECx/EgAxlkQpQ9hYjgGu6EBCPMVPwVIVJqO4XCsMvViHI=",
                "Hello World",
                Bip322SignatureFormat::Simple,
                None,
            ).unwrap()
        );

        assert!(
            verify(
                SEGWIT_ADDRESS,
                "AkgwRQIhAOzyynlqt93lOKJr+wmmxIens//zPzl9tqIOua93wO6MAiBi5n5EyAcPScOjf1lAqIUIQtr3zKNeavYabHyR8eGhowEhAsfxIAMZZEKUPYWI4BruhAQjzFT8FSFSajuFwrDL1Yhy",
                "Hello World",
                Bip322SignatureFormat::Simple,
                None,
            ).unwrap()
        );

        assert!(
            verify(
                SEGWIT_ADDRESS,
                "AkgwRQIhAPkJ1Q4oYS0htvyuSFHLxRQpFAY56b70UvE7Dxazen0ZAiAtZfFz1S6T6I23MWI2lK/pcNTWncuyL8UL+oMdydVgzAEhAsfxIAMZZEKUPYWI4BruhAQjzFT8FSFSajuFwrDL1Yhy",
                "",
                Bip322SignatureFormat::Simple,
                None,
            ).unwrap()
        );
        assert!(
            verify(
                SEGWIT_ADDRESS,
                "AkcwRAIgM2gBAQqvZX15ZiysmKmQpDrG83avLIT492QBzLnQIxYCIBaTpOaD20qRlEylyxFSeEA2ba9YOixpX8z46TSDtS40ASECx/EgAxlkQpQ9hYjgGu6EBCPMVPwVIVJqO4XCsMvViHI=",
                "",
                Bip322SignatureFormat::Simple,
                None,
            ).unwrap()
        );
    }
}
