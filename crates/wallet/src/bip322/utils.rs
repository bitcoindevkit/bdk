//! The utility methods for BIP-322 for message signing
//! according to the BIP-322 standard.

use bitcoin::{
    absolute::LockTime,
    blockdata::opcodes::all::OP_RETURN,
    hashes::{sha256, Hash, HashEngine},
    opcodes::OP_0,
    script::Builder,
    transaction::Version,
    Amount, OutPoint, Psbt, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use std::string::ToString;

use super::BIP322Error;

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
