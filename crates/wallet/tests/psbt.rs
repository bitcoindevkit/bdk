use bdk_wallet::bitcoin::{Amount, FeeRate, Psbt, TxIn};
use bdk_wallet::test_utils::*;
use bdk_wallet::{psbt, KeychainKind, SignOptions};
use core::str::FromStr;

// from bip 174
const PSBT_STR: &str = "cHNidP8BAKACAAAAAqsJSaCMWvfEm4IS9Bfi8Vqz9cM9zxU4IagTn4d6W3vkAAAAAAD+////qwlJoIxa98SbghL0F+LxWrP1wz3PFTghqBOfh3pbe+QBAAAAAP7///8CYDvqCwAAAAAZdqkUdopAu9dAy+gdmI5x3ipNXHE5ax2IrI4kAAAAAAAAGXapFG9GILVT+glechue4O/p+gOcykWXiKwAAAAAAAEHakcwRAIgR1lmF5fAGwNrJZKJSGhiGDR9iYZLcZ4ff89X0eURZYcCIFMJ6r9Wqk2Ikf/REf3xM286KdqGbX+EhtdVRs7tr5MZASEDXNxh/HupccC1AaZGoqg7ECy0OIEhfKaC3Ibi1z+ogpIAAQEgAOH1BQAAAAAXqRQ1RebjO4MsRwUPJNPuuTycA5SLx4cBBBYAFIXRNTfy4mVAWjTbr6nj3aAfuCMIAAAA";

#[test]
#[should_panic(expected = "InputIndexOutOfRange")]
fn test_psbt_malformed_psbt_input_legacy() {
    let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
    let (mut wallet, _) = get_funded_wallet_single(get_test_wpkh());
    let send_to = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), Amount::from_sat(10_000));
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
    let (mut wallet, _) = get_funded_wallet_single(get_test_wpkh());
    let send_to = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), Amount::from_sat(10_000));
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
    let (mut wallet, _) = get_funded_wallet_single(get_test_wpkh());
    let send_to = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), Amount::from_sat(10_000));
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
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let send_to = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.add_recipient(send_to.script_pubkey(), Amount::from_sat(10_000));
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

    let expected_fee_rate = FeeRate::from_sat_per_kwu(310);

    let (mut wallet, _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(expected_fee_rate);
    let mut psbt = builder.finish().unwrap();
    let fee_amount = psbt.fee_amount();
    assert!(fee_amount.is_some());

    let unfinalized_fee_rate = psbt.fee_rate().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let finalized_fee_rate = psbt.fee_rate().unwrap();
    assert!(finalized_fee_rate >= expected_fee_rate);
    assert!(finalized_fee_rate < unfinalized_fee_rate);
}

#[test]
fn test_psbt_fee_rate_with_nonwitness_utxo() {
    use psbt::PsbtUtils;

    let expected_fee_rate = FeeRate::from_sat_per_kwu(310);

    let (mut wallet, _) = get_funded_wallet_single("pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(expected_fee_rate);
    let mut psbt = builder.finish().unwrap();
    let fee_amount = psbt.fee_amount();
    assert!(fee_amount.is_some());
    let unfinalized_fee_rate = psbt.fee_rate().unwrap();

    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
    assert!(finalized);

    let finalized_fee_rate = psbt.fee_rate().unwrap();
    assert!(finalized_fee_rate >= expected_fee_rate);
    assert!(finalized_fee_rate < unfinalized_fee_rate);
}

#[test]
fn test_psbt_fee_rate_with_missing_txout() {
    use psbt::PsbtUtils;

    let expected_fee_rate = FeeRate::from_sat_per_kwu(310);

    let (mut wpkh_wallet,  _) = get_funded_wallet_single("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
    let addr = wpkh_wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wpkh_wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(expected_fee_rate);
    let mut wpkh_psbt = builder.finish().unwrap();

    wpkh_psbt.inputs[0].witness_utxo = None;
    wpkh_psbt.inputs[0].non_witness_utxo = None;
    assert!(wpkh_psbt.fee_amount().is_none());
    assert!(wpkh_psbt.fee_rate().is_none());

    let desc = "pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/0)";
    let change_desc = "pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/1)";
    let (mut pkh_wallet, _) = get_funded_wallet(desc, change_desc);
    let addr = pkh_wallet.peek_address(KeychainKind::External, 0);
    let mut builder = pkh_wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    builder.fee_rate(expected_fee_rate);
    let mut pkh_psbt = builder.finish().unwrap();

    pkh_psbt.inputs[0].non_witness_utxo = None;
    assert!(pkh_psbt.fee_amount().is_none());
    assert!(pkh_psbt.fee_rate().is_none());
}

#[test]
fn test_psbt_multiple_internalkey_signers() {
    use bdk_wallet::signer::{SignerContext, SignerOrdering, SignerWrapper};
    use bdk_wallet::KeychainKind;
    use bitcoin::key::TapTweak;
    use bitcoin::secp256k1::{schnorr, Keypair, Message, Secp256k1, XOnlyPublicKey};
    use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
    use bitcoin::{PrivateKey, TxOut};
    use std::sync::Arc;

    let secp = Secp256k1::new();
    let wif = "cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG";
    let desc = format!("tr({})", wif);
    let prv = PrivateKey::from_wif(wif).unwrap();
    let keypair = Keypair::from_secret_key(&secp, &prv.inner);

    let change_desc = "tr(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
    let (mut wallet, _) = get_funded_wallet(&desc, change_desc);
    let to_spend = wallet.balance().total();
    let send_to = wallet.peek_address(KeychainKind::External, 0);
    let mut builder = wallet.build_tx();
    builder.drain_to(send_to.script_pubkey()).drain_wallet();
    let mut psbt = builder.finish().unwrap();
    let unsigned_tx = psbt.unsigned_tx.clone();

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
    let finalized = wallet.sign(&mut psbt, SignOptions::default()).unwrap();
    assert!(finalized);

    // To verify, we need the signature, message, and pubkey
    let witness = psbt.inputs[0].final_script_witness.as_ref().unwrap();
    assert!(!witness.is_empty());
    let signature = schnorr::Signature::from_slice(witness.iter().next().unwrap()).unwrap();

    // the prevout we're spending
    let prevouts = &[TxOut {
        script_pubkey: send_to.script_pubkey(),
        value: to_spend,
    }];
    let prevouts = Prevouts::All(prevouts);
    let input_index = 0;
    let mut sighash_cache = SighashCache::new(unsigned_tx);
    let sighash = sighash_cache
        .taproot_key_spend_signature_hash(input_index, &prevouts, TapSighashType::Default)
        .unwrap();
    let message = Message::from(sighash);

    // add tweak. this was taken from `signer::sign_psbt_schnorr`
    let keypair = keypair.tap_tweak(&secp, None).to_inner();
    let (xonlykey, _parity) = XOnlyPublicKey::from_keypair(&keypair);

    // Must verify if we used the correct key to sign
    let verify_res = secp.verify_schnorr(&signature, &message, &xonlykey);
    assert!(verify_res.is_ok(), "The wrong internal key was used");
}
