use bdk::bitcoin::TxIn;
use bdk::wallet::AddressIndex;
use bdk::wallet::AddressIndex::New;
use bdk::{psbt, FeeRate, SignOptions};
use bitcoin::psbt::Psbt;
use core::str::FromStr;
mod common;
use common::*;

// from bip 174
const PSBT_STR: &str = "cHNidP8BAKACAAAAAqsJSaCMWvfEm4IS9Bfi8Vqz9cM9zxU4IagTn4d6W3vkAAAAAAD+////qwlJoIxa98SbghL0F+LxWrP1wz3PFTghqBOfh3pbe+QBAAAAAP7///8CYDvqCwAAAAAZdqkUdopAu9dAy+gdmI5x3ipNXHE5ax2IrI4kAAAAAAAAGXapFG9GILVT+glechue4O/p+gOcykWXiKwAAAAAAAEHakcwRAIgR1lmF5fAGwNrJZKJSGhiGDR9iYZLcZ4ff89X0eURZYcCIFMJ6r9Wqk2Ikf/REf3xM286KdqGbX+EhtdVRs7tr5MZASEDXNxh/HupccC1AaZGoqg7ECy0OIEhfKaC3Ibi1z+ogpIAAQEgAOH1BQAAAAAXqRQ1RebjO4MsRwUPJNPuuTycA5SLx4cBBBYAFIXRNTfy4mVAWjTbr6nj3aAfuCMIAAAA";

#[test]
#[should_panic(expected = "InputIndexOutOfRange")]
fn test_psbt_malformed_psbt_input_legacy() {
    let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let send_to = wallet.get_address(AddressIndex::New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), 10_000);
    let mut psbt = builder.finish().unwrap();
    psbt.inputs.push(psbt_bip.inputs[0].clone());
    let options = SignOptions {
        trust_witness_utxo: true,
        ..Default::default()
    };
    let _ = wallet.sign(&mut psbt, options).unwrap();
}

#[test]
#[should_panic(expected = "InputIndexOutOfRange")]
fn test_psbt_malformed_psbt_input_segwit() {
    let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let send_to = wallet.get_address(AddressIndex::New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), 10_000);
    let mut psbt = builder.finish().unwrap();
    psbt.inputs.push(psbt_bip.inputs[1].clone());
    let options = SignOptions {
        trust_witness_utxo: true,
        ..Default::default()
    };
    let _ = wallet.sign(&mut psbt, options).unwrap();
}

#[test]
#[should_panic(expected = "InputIndexOutOfRange")]
fn test_psbt_malformed_tx_input() {
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let send_to = wallet.get_address(AddressIndex::New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), 10_000);
    let mut psbt = builder.finish().unwrap();
    psbt.unsigned_tx.input.push(TxIn::default());
    let options = SignOptions {
        trust_witness_utxo: true,
        ..Default::default()
    };
    let _ = wallet.sign(&mut psbt, options).unwrap();
}

#[test]
fn test_psbt_sign_with_finalized() {
    let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
    let (mut wallet, _) = get_funded_wallet(get_test_wpkh());
    let send_to = wallet.get_address(AddressIndex::New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), 10_000);
    let mut psbt = builder.finish().unwrap();

    // add a finalized input
    psbt.inputs.push(psbt_bip.inputs[0].clone());
    psbt.unsigned_tx
        .input
        .push(psbt_bip.unsigned_tx.input[0].clone());

    let _ = wallet.sign(&mut psbt, SignOptions::default()).unwrap();
}

#[test]
fn test_psbt_fee_rate_with_witness_utxo() {
    use psbt::PsbtUtils;

    let expected_fee_rate = 1.2345;

    let (mut wallet, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
    let mut psbt = builder.finish().unwrap();
    let fee_amount = psbt.fee_amount();
    assert!(fee_amount.is_some());

    let unfinalized_fee_rate = psbt.fee_rate().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let finalized_fee_rate = psbt.fee_rate().unwrap();
    assert!(finalized_fee_rate.as_sat_per_vb() >= expected_fee_rate);
    assert!(finalized_fee_rate.as_sat_per_vb() < unfinalized_fee_rate.as_sat_per_vb());
}

#[test]
fn test_psbt_fee_rate_with_nonwitness_utxo() {
    use psbt::PsbtUtils;

    let expected_fee_rate = 1.2345;

    let (mut wallet, _) = get_funded_wallet("pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.get_address(New);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
    let mut psbt = builder.finish().unwrap();
    let fee_amount = psbt.fee_amount();
    assert!(fee_amount.is_some());
    let unfinalized_fee_rate = psbt.fee_rate().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let finalized_fee_rate = psbt.fee_rate().unwrap();
    assert!(finalized_fee_rate.as_sat_per_vb() >= expected_fee_rate);
    assert!(finalized_fee_rate.as_sat_per_vb() < unfinalized_fee_rate.as_sat_per_vb());
}

#[test]
fn test_psbt_fee_rate_with_missing_txout() {
    use psbt::PsbtUtils;

    let expected_fee_rate = 1.2345;

    let (mut wpkh_wallet,  _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wpkh_wallet.get_address(New);
    let mut builder = wpkh_wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
    let mut wpkh_psbt = builder.finish().unwrap();

    wpkh_psbt.inputs[0].witness_utxo = None;
    wpkh_psbt.inputs[0].non_witness_utxo = None;
    assert!(wpkh_psbt.fee_amount().is_none());
    assert!(wpkh_psbt.fee_rate().is_none());

    let (mut pkh_wallet,  _) = get_funded_wallet("pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = pkh_wallet.get_address(New);
    let mut builder = pkh_wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
    let mut pkh_psbt = builder.finish().unwrap();

    pkh_psbt.inputs[0].non_witness_utxo = None;
    assert!(pkh_psbt.fee_amount().is_none());
    assert!(pkh_psbt.fee_rate().is_none());
}

#[test]
fn test_psbt_multiple_internalkey_signers() {
    use bdk::signer::{SignerContext, SignerOrdering, SignerWrapper};
    use bdk::KeychainKind;
    use bitcoin::{secp256k1::Secp256k1, PrivateKey};
    use miniscript::psbt::PsbtExt;
    use std::sync::Arc;

    let secp = Secp256k1::new();
    let (mut wallet, _) = get_funded_wallet(get_test_tr_single_sig());
    let send_to = wallet.get_address(AddressIndex::New);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), 10_000);
    let mut psbt = builder.finish().unwrap();
    // Adds a signer for the wrong internal key, bdk should not use this key to sign
    wallet.add_signer(
        KeychainKind::External,
        // A signerordering lower than 100, bdk will use this signer first
        SignerOrdering(0),
        Arc::new(SignerWrapper::new(
            PrivateKey::from_wif("5J5PZqvCe1uThJ3FZeUUFLCh2FuK9pZhtEK4MzhNmugqTmxCdwE").unwrap(),
            SignerContext::Tap {
                is_internal_key: true,
            },
        )),
    );
    let _ = wallet.sign(&mut psbt, SignOptions::default()).unwrap();
    // Checks that we signed using the right key
    assert!(
        psbt.finalize_mut(&secp).is_ok(),
        "The wrong internal key was used"
    );
}
