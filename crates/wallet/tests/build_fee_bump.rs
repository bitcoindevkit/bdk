use assert_matches::assert_matches;
use bdk_chain::{ChainPosition, ConfirmationBlockTime};
use bdk_wallet::coin_selection::LargestFirstCoinSelection;
use bdk_wallet::error::CreateTxError;
use bdk_wallet::psbt::PsbtUtils;
use bdk_wallet::test_utils::*;
use bdk_wallet::KeychainKind;
use bitcoin::{
    absolute, transaction, Address, Amount, FeeRate, OutPoint, Sequence, Transaction, TxOut, Weight,
};
use std::str::FromStr;

// The satisfaction size of a P2WPKH is 112 WU =
// 1 (elements in witness) + 1 (OP_PUSH) + 33 (pk) + 1 (OP_PUSH) + 72 (signature + sighash) + 1*4 (script len)
// On the witness itself, we have to push once for the pk (33WU) and once for signature + sighash (72WU), for
// a total of 105 WU.
// Here, we push just once for simplicity, so we have to add an extra byte for the missing
// OP_PUSH.
const P2WPKH_FAKE_WITNESS_SIZE: usize = 106;

macro_rules! assert_fee_rate {
    ($psbt:expr, $fees:expr, $fee_rate:expr $( ,@dust_change $( $dust_change:expr )* )* $( ,@add_signature $( $add_signature:expr )* )* ) => ({
        let psbt = $psbt.clone();
        #[allow(unused_mut)]
        let mut tx = $psbt.clone().extract_tx().expect("failed to extract tx");
        $(
            $( $add_signature )*
                for txin in &mut tx.input {
                    txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
                }
        )*

            #[allow(unused_mut)]
        #[allow(unused_assignments)]
        let mut dust_change = false;
        $(
            $( $dust_change )*
                dust_change = true;
        )*

            let fee_amount = psbt
            .inputs
            .iter()
            .fold(Amount::ZERO, |acc, i| acc + i.witness_utxo.as_ref().unwrap().value)
            - psbt
            .unsigned_tx
            .output
            .iter()
            .fold(Amount::ZERO, |acc, o| acc + o.value);

        assert_eq!(fee_amount, $fees);

        let tx_fee_rate = (fee_amount / tx.weight())
            .to_sat_per_kwu();
        let fee_rate = $fee_rate.to_sat_per_kwu();
        let half_default = FeeRate::BROADCAST_MIN.checked_div(2)
            .unwrap()
            .to_sat_per_kwu();

        if !dust_change {
            assert!(tx_fee_rate >= fee_rate && tx_fee_rate - fee_rate < half_default, "Expected fee rate of {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
        } else {
            assert!(tx_fee_rate >= fee_rate, "Expected fee rate of at least {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
        }
    });
}

macro_rules! check_fee {
    ($wallet:expr, $psbt: expr) => {{
        let tx = $psbt.clone().extract_tx().expect("failed to extract tx");
        let tx_fee = $wallet.calculate_fee(&tx).ok();
        assert_eq!(tx_fee, $psbt.fee_amount());
        tx_fee
    }};
}

#[test]
#[should_panic(expected = "IrreplaceableTransaction")]
fn test_bump_fee_irreplaceable_tx() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    builder.set_exact_sequence(Sequence(0xFFFFFFFE));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    wallet.build_fee_bump(txid).unwrap().finish().unwrap();
}

#[test]
#[should_panic(expected = "TransactionConfirmed")]
fn test_bump_fee_confirmed_tx() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();

    insert_tx(&mut wallet, tx);

    let anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().get(42).unwrap().block_id(),
        confirmation_time: 42_000,
    };
    insert_anchor(&mut wallet, txid, anchor);

    wallet.build_fee_bump(txid).unwrap().finish().unwrap();
}

#[test]
fn test_bump_fee_low_fee_rate() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));

    let psbt = builder.finish().unwrap();
    let feerate = psbt.fee_rate().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::BROADCAST_MIN);
    let res = builder.finish();
    assert_matches!(
        res,
        Err(CreateTxError::FeeRateTooLow { .. }),
        "expected FeeRateTooLow error"
    );

    let required = feerate.to_sat_per_kwu() + 250; // +1 sat/vb
    let sat_vb = required as f64 / 250.0;
    let expect = format!("Fee rate too low: required {} sat/vb", sat_vb);
    assert_eq!(res.unwrap_err().to_string(), expect);
}

#[test]
#[should_panic(expected = "FeeTooLow")]
fn test_bump_fee_low_abs() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::from_sat(10));
    builder.finish().unwrap();
}

#[test]
#[should_panic(expected = "FeeTooLow")]
fn test_bump_fee_zero_abs() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = wallet.next_unused_address(KeychainKind::External);
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::ZERO);
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_reduce_change() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(25_000));
    let psbt = builder.finish().unwrap();
    let original_sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let original_fee = check_fee!(wallet, psbt);

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let feerate = FeeRate::from_sat_per_kwu(625); // 2.5 sat/vb
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(feerate);
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert_eq!(
        sent_received.1 + fee.unwrap_or(Amount::ZERO),
        original_sent_received.1 + original_fee.unwrap_or(Amount::ZERO)
    );
    assert!(fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(25_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), feerate, @add_signature);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::from_sat(200));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert_eq!(
        sent_received.1 + fee.unwrap_or(Amount::ZERO),
        original_sent_received.1 + original_fee.unwrap_or(Amount::ZERO)
    );
    assert!(
        fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO),
        "{} > {}",
        fee.unwrap_or(Amount::ZERO),
        original_fee.unwrap_or(Amount::ZERO)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(25_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(200));
}

#[test]
fn test_bump_fee_reduce_single_recipient() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();
    let tx = psbt.clone().extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let original_fee = check_fee!(wallet, psbt);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let feerate = FeeRate::from_sat_per_kwu(625); // 2.5 sat/vb
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_rate(feerate)
        // remove original tx drain_to address and amount
        .set_recipients(Vec::new())
        // set back original drain_to address
        .drain_to(addr.script_pubkey())
        // drain wallet output amount will be re-calculated with new fee rate
        .drain_wallet();
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert!(fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO));

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output[0].value + fee.unwrap_or(Amount::ZERO),
        sent_received.0
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), feerate, @add_signature);
}

#[test]
fn test_bump_fee_absolute_reduce_single_recipient() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.drain_to(addr.script_pubkey()).drain_wallet();
    let psbt = builder.finish().unwrap();
    let original_fee = check_fee!(wallet, psbt);
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_absolute(Amount::from_sat(300))
        // remove original tx drain_to address and amount
        .set_recipients(Vec::new())
        // set back original drain_to address
        .drain_to(addr.script_pubkey())
        // drain wallet output amount will be re-calculated with new fee rate
        .drain_wallet();
    let psbt = builder.finish().unwrap();
    let tx = &psbt.unsigned_tx;
    let sent_received = wallet.sent_and_received(tx);
    let fee = check_fee!(wallet, psbt);

    assert_eq!(sent_received.0, original_sent_received.0);
    assert!(fee.unwrap_or(Amount::ZERO) > original_fee.unwrap_or(Amount::ZERO));

    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output[0].value + fee.unwrap_or(Amount::ZERO),
        sent_received.0
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(300));
}

#[test]
fn test_bump_fee_drain_wallet() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    // receive an extra tx so that our wallet has two utxos.
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx.clone());
    let anchor = ConfirmationBlockTime {
        block_id: wallet.latest_checkpoint().block_id(),
        confirmation_time: 42_000,
    };
    insert_anchor(&mut wallet, txid, anchor);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();

    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(OutPoint {
            txid: tx.compute_txid(),
            vout: 0,
        })
        .unwrap()
        .manually_selected_only();
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);

    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    assert_eq!(original_sent_received.0, Amount::from_sat(25_000));

    // for the new feerate, it should be enough to reduce the output, but since we specify
    // `drain_wallet` we expect to spend everything
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .drain_wallet()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(5));
    let psbt = builder.finish().unwrap();
    let sent_received = wallet.sent_and_received(&psbt.extract_tx().expect("failed to extract tx"));

    assert_eq!(sent_received.0, Amount::from_sat(75_000));
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_bump_fee_remove_output_manually_selected_only() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    // receive an extra tx so that our wallet has two utxos. then we manually pick only one of
    // them, and make sure that `bump_fee` doesn't try to add more. This fails because we've
    // told the wallet it's not allowed to add more inputs AND it can't reduce the value of the
    // existing output. In other words, bump_fee + manually_selected_only is always an error
    // unless there is a change output.
    let init_tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };

    let position: ChainPosition<ConfirmationBlockTime> =
        wallet.transactions().last().unwrap().chain_position;
    insert_tx(&mut wallet, init_tx.clone());
    match position {
        ChainPosition::Confirmed { anchor, .. } => {
            insert_anchor(&mut wallet, init_tx.compute_txid(), anchor)
        }
        other => panic!("all wallet txs must be confirmed: {:?}", other),
    }

    let outpoint = OutPoint {
        txid: init_tx.compute_txid(),
        vout: 0,
    };
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(outpoint)
        .unwrap()
        .manually_selected_only();
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);
    assert_eq!(original_sent_received.0, Amount::from_sat(25_000));

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .manually_selected_only()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(255));
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let init_tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: wallet
                .next_unused_address(KeychainKind::External)
                .script_pubkey(),
            value: Amount::from_sat(25_000),
        }],
    };
    let txid = init_tx.compute_txid();
    let pos: ChainPosition<ConfirmationBlockTime> =
        wallet.transactions().last().unwrap().chain_position;
    insert_tx(&mut wallet, init_tx);
    match pos {
        ChainPosition::Confirmed { anchor, .. } => insert_anchor(&mut wallet, txid, anchor),
        other => panic!("all wallet txs must be confirmed: {:?}", other),
    }

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_details = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb_unchecked(50));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);
    assert_eq!(
        sent_received.0,
        original_details.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(50), @add_signature);
}

#[test]
fn test_bump_fee_absolute_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    receive_output_in_latest_block(&mut wallet, 25_000);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_absolute(Amount::from_sat(6_000));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(6_000));
}

#[test]
fn test_bump_fee_no_change_add_input_and_change() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let op = receive_output_in_latest_block(&mut wallet, 25_000);

    // initially make a tx without change by using `drain_to`
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder
        .drain_to(addr.script_pubkey())
        .add_utxo(op)
        .unwrap()
        .manually_selected_only();
    let psbt = builder.finish().unwrap();
    let original_sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let original_fee = check_fee!(wallet, psbt);

    let tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    // Now bump the fees, the wallet should add an extra input and a change output, and leave
    // the original output untouched.
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb_unchecked(50));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    let original_send_all_amount = original_sent_received.0 - original_fee.unwrap_or(Amount::ZERO);
    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(50_000)
    );
    assert_eq!(
        sent_received.1,
        Amount::from_sat(75_000) - original_send_all_amount - fee.unwrap_or(Amount::ZERO)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        original_send_all_amount
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(75_000) - original_send_all_amount - fee.unwrap_or(Amount::ZERO)
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(50), @add_signature);
}

#[test]
fn test_bump_fee_add_input_change_dust() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    receive_output_in_latest_block(&mut wallet, 25_000);
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let original_sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let original_fee = check_fee!(wallet, psbt);

    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // to get realistic weight
    }
    let original_tx_weight = tx.weight();
    assert_eq!(tx.input.len(), 1);
    assert_eq!(tx.output.len(), 2);
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    // We set a fee high enough that during rbf we are forced to add
    // a new input and also that we have to remove the change
    // that we had previously

    // We calculate the new weight as:
    //   original weight
    // + extra input weight: 160 WU = (32 (prevout) + 4 (vout) + 4 (nsequence)) * 4
    // + input satisfaction weight: 112 WU = 106 (witness) + 2 (witness len) + (1 (script len)) * 4
    // - change output weight: 124 WU = (8 (value) + 1 (script len) + 22 (script)) * 4
    let new_tx_weight =
        original_tx_weight + Weight::from_wu(160) + Weight::from_wu(112) - Weight::from_wu(124);
    // two inputs (50k, 25k) and one output (45k) - epsilon
    // We use epsilon here to avoid asking for a slightly too high feerate
    let fee_abs = 50_000 + 25_000 - 45_000 - 10;
    builder.fee_rate(Amount::from_sat(fee_abs) / new_tx_weight);
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        original_sent_received.1,
        Amount::from_sat(5_000) - original_fee.unwrap_or(Amount::ZERO)
    );

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(30_000));
    assert_eq!(sent_received.1, Amount::ZERO);

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 1);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(45_000)
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(140), @dust_change, @add_signature);
}

#[test]
fn test_bump_fee_force_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let incoming_op = receive_output_in_latest_block(&mut wallet, 25_000);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx.clone());
    // the new fee_rate is low enough that just reducing the change would be fine, but we force
    // the addition of an extra input with `add_utxo()`
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .add_utxo(incoming_op)
        .unwrap()
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(5));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_fee_rate!(psbt, fee.unwrap_or(Amount::ZERO), FeeRate::from_sat_per_vb_unchecked(5), @add_signature);
}

#[test]
fn test_bump_fee_absolute_force_add_input() {
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let incoming_op = receive_output_in_latest_block(&mut wallet, 25_000);

    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx().coin_selection(LargestFirstCoinSelection);
    builder.add_recipient(addr.script_pubkey(), Amount::from_sat(45_000));
    let psbt = builder.finish().unwrap();
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let original_sent_received = wallet.sent_and_received(&tx);
    let txid = tx.compute_txid();
    // skip saving the new utxos, we know they can't be used anyways
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx.clone());

    // the new fee_rate is low enough that just reducing the change would be fine, but we force
    // the addition of an extra input with `add_utxo()`
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .add_utxo(incoming_op)
        .unwrap()
        .fee_absolute(Amount::from_sat(250));
    let psbt = builder.finish().unwrap();
    let sent_received =
        wallet.sent_and_received(&psbt.clone().extract_tx().expect("failed to extract tx"));
    let fee = check_fee!(wallet, psbt);

    assert_eq!(
        sent_received.0,
        original_sent_received.0 + Amount::from_sat(25_000)
    );
    assert_eq!(
        fee.unwrap_or(Amount::ZERO) + sent_received.1,
        Amount::from_sat(30_000)
    );

    let tx = &psbt.unsigned_tx;
    assert_eq!(tx.input.len(), 2);
    assert_eq!(tx.output.len(), 2);
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey == addr.script_pubkey())
            .unwrap()
            .value,
        Amount::from_sat(45_000)
    );
    assert_eq!(
        tx.output
            .iter()
            .find(|txout| txout.script_pubkey != addr.script_pubkey())
            .unwrap()
            .value,
        sent_received.1
    );

    assert_eq!(fee.unwrap_or(Amount::ZERO), Amount::from_sat(250));
}

#[test]
#[should_panic(expected = "InsufficientFunds")]
fn test_bump_fee_unconfirmed_inputs_only() {
    // We try to bump the fee, but:
    // - We can't reduce the change, as we have no change
    // - All our UTXOs are unconfirmed
    // So, we fail with "InsufficientFunds", as per RBF rule 2:
    // The replacement transaction may only include an unconfirmed input
    // if that input was included in one of the original transactions.
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    let mut builder = wallet.build_tx();
    builder.drain_wallet().drain_to(addr.script_pubkey());
    let psbt = builder.finish().unwrap();
    // Now we receive one transaction with 0 confirmations. We won't be able to use that for
    // fee bumping, as it's still unconfirmed!
    receive_output(&mut wallet, 25_000, ReceiveTo::Mempool(0));
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx);
    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder.fee_rate(FeeRate::from_sat_per_vb_unchecked(25));
    builder.finish().unwrap();
}

#[test]
fn test_bump_fee_unconfirmed_input() {
    // We create a tx draining the wallet and spending one confirmed
    // and one unconfirmed UTXO. We check that we can fee bump normally
    // (BIP125 rule 2 only apply to newly added unconfirmed input, you can
    // always fee bump with an unconfirmed input if it was included in the
    // original transaction)
    let (mut wallet, _) = get_funded_wallet_wpkh();
    let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX")
        .unwrap()
        .assume_checked();
    // We receive a tx with 0 confirmations, which will be used as an input
    // in the drain tx.
    receive_output(&mut wallet, 25_000, ReceiveTo::Mempool(0));
    let mut builder = wallet.build_tx();
    builder.drain_wallet().drain_to(addr.script_pubkey());
    let psbt = builder.finish().unwrap();
    let mut tx = psbt.extract_tx().expect("failed to extract tx");
    let txid = tx.compute_txid();
    for txin in &mut tx.input {
        txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
    }
    insert_tx(&mut wallet, tx);

    let mut builder = wallet.build_fee_bump(txid).unwrap();
    builder
        .fee_rate(FeeRate::from_sat_per_vb_unchecked(15))
        // remove original tx drain_to address and amount
        .set_recipients(Vec::new())
        // set back original drain_to address
        .drain_to(addr.script_pubkey())
        // drain wallet output amount will be re-calculated with new fee rate
        .drain_wallet();
    builder.finish().unwrap();
}
