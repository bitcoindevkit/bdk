#![cfg(feature = "std")]

use bdk_chain::{local_chain::LocalChain, AncestorPackage, CanonicalizationParams, TxGraph};
use bdk_core::{BlockId, ConfirmationBlockTime};
use bitcoin::{
    absolute, hashes::Hash, transaction, Amount, BlockHash, FeeRate, OutPoint, ScriptBuf,
    Transaction, TxIn, TxOut, Txid, Weight,
};
use std::collections::BTreeMap;

fn make_tx(inputs: &[OutPoint], output_values: &[Amount]) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: inputs
            .iter()
            .map(|prev| TxIn {
                previous_output: *prev,
                ..Default::default()
            })
            .collect(),
        output: output_values
            .iter()
            .map(|&value| TxOut {
                value,
                script_pubkey: ScriptBuf::new(),
            })
            .collect(),
    }
}

fn block_id(height: u32) -> BlockId {
    BlockId {
        height,
        hash: BlockHash::from_byte_array([height as u8; 32]),
    }
}

fn anchor(height: u32) -> ConfirmationBlockTime {
    ConfirmationBlockTime {
        block_id: block_id(height),
        confirmation_time: 123456,
    }
}

fn build_view(
    graph: &TxGraph<ConfirmationBlockTime>,
    chain: &LocalChain,
) -> bdk_chain::CanonicalView<ConfirmationBlockTime> {
    let tip = chain.tip().block_id();
    graph
        .try_canonical_view(chain, tip, CanonicalizationParams::default())
        .expect("infallible chain oracle")
}

fn build_packages(
    graph: &TxGraph<ConfirmationBlockTime>,
    chain: &LocalChain,
) -> BTreeMap<OutPoint, AncestorPackage> {
    build_view(graph, chain).ancestor_packages()
}

/// Set up a chain with a confirmed coinbase (1 BTC) at height 1.
fn setup() -> (LocalChain, TxGraph<ConfirmationBlockTime>, Txid) {
    let mut graph = TxGraph::<ConfirmationBlockTime>::default();
    let chain =
        LocalChain::from_blocks([(0, BlockHash::all_zeros()), (1, block_id(1).hash)].into())
            .unwrap();

    let coinbase = make_tx(&[OutPoint::null()], &[Amount::from_sat(100_000_000)]);
    let coinbase_txid = coinbase.compute_txid();
    let _ = graph.insert_tx(coinbase);
    let _ = graph.insert_anchor(coinbase_txid, anchor(1));

    (chain, graph, coinbase_txid)
}

#[test]
fn single_unconfirmed_parent() {
    let (chain, mut graph, coinbase_txid) = setup();

    // fee = 100_000_000 - 99_990_000 - 9_000 = 1_000
    let tx_1 = make_tx(
        &[OutPoint::new(coinbase_txid, 0)],
        &[Amount::from_sat(99_990_000), Amount::from_sat(9_000)],
    );
    let txid_1 = tx_1.compute_txid();
    let weight_1 = tx_1.weight();
    let _ = graph.insert_tx(tx_1);
    let _ = graph.insert_seen_at(txid_1, 1000);

    let packages = build_packages(&graph, &chain);

    let pkg_0 = packages.get(&OutPoint::new(txid_1, 0)).unwrap();
    let pkg_1 = packages.get(&OutPoint::new(txid_1, 1)).unwrap();

    assert_eq!(pkg_0, pkg_1, "sibling UTXOs must have identical packages");
    assert_eq!(pkg_0.fee, Amount::from_sat(1_000));
    assert_eq!(pkg_0.weight, weight_1);
}

#[test]
fn two_level_unconfirmed_chain() {
    let (chain, mut graph, coinbase_txid) = setup();

    // tx_1: fee = 5_000
    let tx_1 = make_tx(
        &[OutPoint::new(coinbase_txid, 0)],
        &[Amount::from_sat(99_995_000)],
    );
    let txid_1 = tx_1.compute_txid();
    let weight_1 = tx_1.weight();
    let _ = graph.insert_tx(tx_1);
    let _ = graph.insert_seen_at(txid_1, 1000);

    // tx_2: spends TX1:0, fee = 5_000
    let tx_2 = make_tx(&[OutPoint::new(txid_1, 0)], &[Amount::from_sat(99_990_000)]);
    let txid_2 = tx_2.compute_txid();
    let weight_2 = tx_2.weight();
    let _ = graph.insert_tx(tx_2);
    let _ = graph.insert_seen_at(txid_2, 1001);

    let packages = build_packages(&graph, &chain);

    assert!(!packages.contains_key(&OutPoint::new(txid_1, 0)));

    let pkg = packages.get(&OutPoint::new(txid_2, 0)).unwrap();
    assert_eq!(pkg.fee, Amount::from_sat(10_000));
    assert_eq!(pkg.weight, weight_1 + weight_2);
}

#[test]
fn stops_at_confirmed_boundary() {
    let (chain, mut graph, coinbase_txid) = setup();

    // Confirmed tx_1
    let tx_1 = make_tx(
        &[OutPoint::new(coinbase_txid, 0)],
        &[Amount::from_sat(99_999_000)],
    );
    let txid_1 = tx_1.compute_txid();
    let _ = graph.insert_tx(tx_1);
    let _ = graph.insert_anchor(txid_1, anchor(1));

    // Unconfirmed tx_2: spends confirmed TX1:0, fee = 2_000
    let tx_2 = make_tx(&[OutPoint::new(txid_1, 0)], &[Amount::from_sat(99_997_000)]);
    let txid_2 = tx_2.compute_txid();
    let weight_2 = tx_2.weight();
    let _ = graph.insert_tx(tx_2);
    let _ = graph.insert_seen_at(txid_2, 1000);

    // Unconfirmed tx_3: spends TX2:0, fee = 5_000
    let tx_3 = make_tx(&[OutPoint::new(txid_2, 0)], &[Amount::from_sat(99_992_000)]);
    let txid_3 = tx_3.compute_txid();
    let weight_3 = tx_3.weight();
    let _ = graph.insert_tx(tx_3);
    let _ = graph.insert_seen_at(txid_3, 1001);

    let packages = build_packages(&graph, &chain);
    let pkg = packages.get(&OutPoint::new(txid_3, 0)).unwrap();

    assert_eq!(pkg.fee, Amount::from_sat(7_000));
    assert_eq!(pkg.weight, weight_2 + weight_3);
}

#[test]
fn shared_ancestor_counted_once() {
    let (chain, mut graph, coinbase_txid) = setup();

    // Unconfirmed TX0: fee = 10_000_000
    let tx_0 = make_tx(
        &[OutPoint::new(coinbase_txid, 0)],
        &[Amount::from_sat(50_000_000), Amount::from_sat(40_000_000)],
    );
    let txid_0 = tx_0.compute_txid();
    let weight_0 = tx_0.weight();
    let _ = graph.insert_tx(tx_0);
    let _ = graph.insert_seen_at(txid_0, 1000);

    // Unconfirmed TX1: spends TX0:0, fee = 5_000
    let tx_1 = make_tx(&[OutPoint::new(txid_0, 0)], &[Amount::from_sat(49_995_000)]);
    let txid_1 = tx_1.compute_txid();
    let weight_1 = tx_1.weight();
    let _ = graph.insert_tx(tx_1);
    let _ = graph.insert_seen_at(txid_1, 1001);

    // Unconfirmed TX2: spends TX0:1, fee = 5_000
    let tx_2 = make_tx(&[OutPoint::new(txid_0, 1)], &[Amount::from_sat(39_995_000)]);
    let txid_2 = tx_2.compute_txid();
    let weight_2 = tx_2.weight();
    let _ = graph.insert_tx(tx_2);
    let _ = graph.insert_seen_at(txid_2, 1002);

    // Unconfirmed TX3: spends TX1:0 and TX2:0, fee = 10_000
    let tx_3 = make_tx(
        &[OutPoint::new(txid_1, 0), OutPoint::new(txid_2, 0)],
        &[Amount::from_sat(89_980_000)],
    );
    let txid_3 = tx_3.compute_txid();
    let weight_3 = tx_3.weight();
    let _ = graph.insert_tx(tx_3);
    let _ = graph.insert_seen_at(txid_3, 1003);

    let packages = build_packages(&graph, &chain);
    let pkg = packages.get(&OutPoint::new(txid_3, 0)).unwrap();

    // TX0 counted once despite being ancestor of both TX1 and TX2.
    assert_eq!(
        pkg.fee,
        Amount::from_sat(10_000_000 + 5_000 + 5_000 + 10_000)
    );
    assert_eq!(pkg.weight, weight_0 + weight_1 + weight_2 + weight_3);
}

#[test]
fn aggregate_deduplicates_shared_ancestors() {
    let (chain, mut graph, coinbase_txid) = setup();

    // Unconfirmed tx_0: two outputs, fee = 10_000
    let tx_0 = make_tx(
        &[OutPoint::new(coinbase_txid, 0)],
        &[Amount::from_sat(50_000_000), Amount::from_sat(49_990_000)],
    );
    let txid_0 = tx_0.compute_txid();
    let weight_0 = tx_0.weight();
    let _ = graph.insert_tx(tx_0);
    let _ = graph.insert_seen_at(txid_0, 1000);

    // Unconfirmed tx_1: spends tx_0:0, fee = 5_000
    let tx_1 = make_tx(&[OutPoint::new(txid_0, 0)], &[Amount::from_sat(49_995_000)]);
    let txid_1 = tx_1.compute_txid();
    let weight_1 = tx_1.weight();
    let _ = graph.insert_tx(tx_1);
    let _ = graph.insert_seen_at(txid_1, 1001);

    // Unconfirmed tx_2: spends tx_0:1, fee = 5_000
    let tx_2 = make_tx(&[OutPoint::new(txid_0, 1)], &[Amount::from_sat(49_985_000)]);
    let txid_2 = tx_2.compute_txid();
    let weight_2 = tx_2.weight();
    let _ = graph.insert_tx(tx_2);
    let _ = graph.insert_seen_at(txid_2, 1002);

    let op_1 = OutPoint::new(txid_1, 0);
    let op_2 = OutPoint::new(txid_2, 0);

    let view = build_view(&graph, &chain);
    let packages = view.ancestor_packages();

    // Per-outpoint: each independently includes tx_0.
    let sum_fee = packages[&op_1].fee + packages[&op_2].fee;
    assert_eq!(sum_fee, Amount::from_sat(30_000));

    // Aggregate: tx_0 counted once.
    let agg = view
        .aggregate_ancestor_package([OutPoint::new(txid_1, 0), OutPoint::new(txid_2, 0)])
        .unwrap();

    assert_eq!(agg.fee, Amount::from_sat(10_000 + 5_000 + 5_000));
    assert_eq!(agg.weight, weight_0 + weight_1 + weight_2);
}

#[test]
fn fee_deficit_at_various_feerates() {
    let pkg = AncestorPackage {
        weight: Weight::from_wu(1000),
        fee: Amount::from_sat(250),
    };

    // At 1 sat/vbyte (0.25 sat/wu): required = 250. Met exactly.
    let rate_1 = FeeRate::from_sat_per_vb_unchecked(1);
    assert_eq!(pkg.fee_deficit(rate_1), Amount::ZERO);

    // At 2 sat/vbyte (0.5 sat/wu): required = 500. Deficit = 250.
    let rate_2 = FeeRate::from_sat_per_vb_unchecked(2);
    assert_eq!(pkg.fee_deficit(rate_2), Amount::from_sat(250));

    // At 10 sat/vbyte (2.5 sat/wu): required = 2500. Deficit = 2250.
    let rate_10 = FeeRate::from_sat_per_vb_unchecked(10);
    assert_eq!(pkg.fee_deficit(rate_10), Amount::from_sat(2250));

    // At 0 sat/vbyte: required = 0. Already met.
    assert_eq!(pkg.fee_deficit(FeeRate::ZERO), Amount::ZERO);
}
