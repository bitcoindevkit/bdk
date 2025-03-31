use bdk_wallet::psbt::PsbtUtils;
use bdk_wallet::signer::SignOptions;
use bdk_wallet::test_utils::*;
use bdk_wallet::tx_builder::AddForeignUtxoError;
use bdk_wallet::KeychainKind;
use bitcoin::psbt;
use bitcoin::{Address, Amount};
use std::str::FromStr;

mod common;

#[test]
fn test_add_foreign_utxo() {
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, _) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo = wallet2.list_unspent().next().expect("must take!");
    let foreign_utxo_satisfaction = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let psbt_input = psbt::Input {
        witness_utxo: Some(utxo.txout.clone()),
        ..Default::default()
    };

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(60_000))
        .only_witness_utxo()
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let mut psbt = builder.finish().unwrap();
    wallet1.insert_txout(utxo.outpoint, utxo.txout);
    let fee = check_fee!(wallet1, psbt);
    let sent_received =
        wallet1.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));

    assert_eq!(
        (sent_received.0 - sent_received.1),
        Amount::from_sat(10_000) + fee.unwrap_or(Amount::ZERO),
        "we should have only net spent ~10_000"
    );

    assert!(
        psbt.unsigned_tx
            .input
            .iter()
            .any(|input| input.previous_output == utxo.outpoint),
        "foreign_utxo should be in there"
    );

    let finished = wallet1
        .sign(
            &mut psbt,
            SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )
        .unwrap();

    assert!(
        !finished,
        "only one of the inputs should have been signed so far"
    );

    let finished = wallet2
        .sign(
            &mut psbt,
            SignOptions {
                trust_witness_utxo: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(finished, "all the inputs should have been signed now");
}

#[test]
fn test_calculate_fee_with_missing_foreign_utxo() {
    use bdk_chain::tx_graph::CalculateFeeError;
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, _) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo = wallet2.list_unspent().next().expect("must take!");
    let foreign_utxo_satisfaction = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let psbt_input = psbt::Input {
        witness_utxo: Some(utxo.txout.clone()),
        ..Default::default()
    };

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(60_000))
        .only_witness_utxo()
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let res = wallet1.calculate_fee(&tx);
    assert!(
        matches!(res, Err(CalculateFeeError::MissingTxOut(outpoints)) if outpoints[0] == utxo.outpoint)
    );
}

#[test]
fn test_add_foreign_utxo_invalid_psbt_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let outpoint = wallet.list_unspent().next().expect("must exist").outpoint;
    let foreign_utxo_satisfaction = wallet
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let mut builder = wallet.build_tx();
    let result =
        builder.add_foreign_utxo(outpoint, psbt::Input::default(), foreign_utxo_satisfaction);
    assert!(matches!(result, Err(AddForeignUtxoError::MissingUtxo)));
}

#[test]
fn test_add_foreign_utxo_where_outpoint_doesnt_match_psbt_input() {
    let (mut wallet1, txid1) = get_funded_wallet_wpkh();
    let (wallet2, txid2) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

    let utxo2 = wallet2.list_unspent().next().unwrap();
    let tx1 = wallet1.get_tx(txid1).unwrap().tx_node.tx.clone();
    let tx2 = wallet2.get_tx(txid2).unwrap().tx_node.tx.clone();

    let satisfaction_weight = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    let mut builder = wallet1.build_tx();
    assert!(
        builder
            .add_foreign_utxo(
                utxo2.outpoint,
                psbt::Input {
                    non_witness_utxo: Some(tx1.as_ref().clone()),
                    ..Default::default()
                },
                satisfaction_weight
            )
            .is_err(),
        "should fail when outpoint doesn't match psbt_input"
    );
    assert!(
        builder
            .add_foreign_utxo(
                utxo2.outpoint,
                psbt::Input {
                    non_witness_utxo: Some(tx2.as_ref().clone()),
                    ..Default::default()
                },
                satisfaction_weight
            )
            .is_ok(),
        "should be ok when outpoint does match psbt_input"
    );
}

#[test]
fn test_add_foreign_utxo_only_witness_utxo() {
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, txid2) =
        get_funded_wallet_single("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo2 = wallet2.list_unspent().next().unwrap();

    let satisfaction_weight = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    {
        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), Amount::from_sat(60_000));

        let psbt_input = psbt::Input {
            witness_utxo: Some(utxo2.txout.clone()),
            ..Default::default()
        };
        builder
            .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
            .unwrap();
        assert!(
            builder.finish().is_err(),
            "psbt_input with witness_utxo should fail with only witness_utxo"
        );
    }

    {
        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), Amount::from_sat(60_000));

        let psbt_input = psbt::Input {
            witness_utxo: Some(utxo2.txout.clone()),
            ..Default::default()
        };
        builder
            .only_witness_utxo()
            .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
            .unwrap();
        assert!(
            builder.finish().is_ok(),
            "psbt_input with just witness_utxo should succeed when `only_witness_utxo` is enabled"
        );
    }

    {
        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), Amount::from_sat(60_000));

        let tx2 = wallet2.get_tx(txid2).unwrap().tx_node.tx;
        let psbt_input = psbt::Input {
            non_witness_utxo: Some(tx2.as_ref().clone()),
            ..Default::default()
        };
        builder
            .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
            .unwrap();
        assert!(
            builder.finish().is_ok(),
            "psbt_input with non_witness_utxo should succeed by default"
        );
    }
}

#[test]
fn test_taproot_foreign_utxo() {
    let (mut wallet1, _) = get_funded_wallet_wpkh();
    let (wallet2, _) = get_funded_wallet_single(get_test_tr_single_sig());

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let utxo = wallet2.list_unspent().next().unwrap();
    let psbt_input = wallet2.get_psbt_input(utxo.clone(), None, false).unwrap();
    let foreign_utxo_satisfaction = wallet2
        .public_descriptor(KeychainKind::External)
        .max_weight_to_satisfy()
        .unwrap();

    assert!(
        psbt_input.non_witness_utxo.is_none(),
        "`non_witness_utxo` should never be populated for taproot"
    );

    let mut builder = wallet1.build_tx();
    builder
        .add_recipient(addr.script_pubkey(), Amount::from_sat(60_000))
        .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
        .unwrap();
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet1.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    wallet1.insert_txout(utxo.outpoint, utxo.txout);
    let fee = check_fee!(wallet1, psbt);

    assert_eq!(
        sent_received.0 - sent_received.1,
        Amount::from_sat(10_000) + fee.unwrap_or(Amount::ZERO),
        "we should have only net spent ~10_000"
    );

    assert!(
        psbt.unsigned_tx
            .input
            .iter()
            .any(|input| input.previous_output == utxo.outpoint),
        "foreign_utxo should be in there"
    );
}
