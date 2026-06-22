#![cfg(feature = "miniscript")]

use std::collections::{BTreeMap, BTreeSet, HashSet};

use bdk_chain::{local_chain::LocalChain, CanonicalTx, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};

fn out(val: u64) -> TxOut {
    TxOut {
        value: Amount::from_sat(val),
        script_pubkey: ScriptBuf::new(),
    }
}

fn txin(op: OutPoint) -> TxIn {
    TxIn {
        previous_output: op,
        ..Default::default()
    }
}

/// `map` that contributes the visited txid into a `BTreeSet`. A node's final accumulator is then
/// the set of every txid in its descendant-closure within the traversed set, plus itself.
fn collect_self<P>(ctx: CanonicalTx<P>) -> BTreeSet<Txid> {
    [ctx.txid].into_iter().collect()
}

/// Builds a diamond of confirmed transactions and returns the chain, graph and the four txs.
///
/// ```text
///        tx_a            (2 outputs)
///       /    \
///    tx_b    tx_c
///       \    /
///        tx_d            (root we walk ancestors from)
/// ```
fn diamond() -> (LocalChain, TxGraph<ConfirmationBlockTime>, [Transaction; 4]) {
    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("block0")),
        (1, hash!("block1")),
        (2, hash!("block2")),
        (3, hash!("block3")),
        (4, hash!("block4")),
        (5, hash!("block5")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    let tx_a = Transaction {
        input: vec![txin(OutPoint::new(hash!("external"), 0))],
        output: vec![out(10_000), out(10_000)],
        ..new_tx(0)
    };
    let txid_a = tx_a.compute_txid();

    let tx_b = Transaction {
        input: vec![txin(OutPoint::new(txid_a, 0))],
        output: vec![out(9_000)],
        ..new_tx(1)
    };
    let txid_b = tx_b.compute_txid();

    let tx_c = Transaction {
        input: vec![txin(OutPoint::new(txid_a, 1))],
        output: vec![out(9_000)],
        ..new_tx(2)
    };
    let txid_c = tx_c.compute_txid();

    let tx_d = Transaction {
        input: vec![
            txin(OutPoint::new(txid_b, 0)),
            txin(OutPoint::new(txid_c, 0)),
        ],
        output: vec![out(15_000)],
        ..new_tx(3)
    };
    let txid_d = tx_d.compute_txid();

    for (txid, tx, height) in [
        (txid_a, &tx_a, 1u32),
        (txid_b, &tx_b, 2),
        (txid_c, &tx_c, 2),
        (txid_d, &tx_d, 3),
    ] {
        let _ = tx_graph.insert_tx(tx.clone());
        let _ = tx_graph.insert_anchor(
            txid,
            ConfirmationBlockTime {
                block_id: chain.get(height).unwrap().block_id(),
                confirmation_time: 100,
            },
        );
    }

    (chain, tx_graph, [tx_a, tx_b, tx_c, tx_d])
}

#[test]
fn ancestors_reverse_topological_and_merged() {
    let (chain, tx_graph, [tx_a, tx_b, tx_c, tx_d]) = diamond();
    let (txid_a, txid_b, txid_c, txid_d) = (
        tx_a.compute_txid(),
        tx_b.compute_txid(),
        tx_c.compute_txid(),
        tx_d.compute_txid(),
    );

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    let result: Vec<_> = view
        .ancestors([txid_d], collect_self, |_ctx| true)
        .collect();

    // Three ancestors (root tx_d is not yielded), each exactly once.
    let order: Vec<Txid> = result.iter().map(|(_, ct)| ct.txid).collect();
    assert_eq!(order.len(), 3);
    assert_eq!(order.iter().copied().collect::<HashSet<_>>().len(), 3);

    let pos = |t: Txid| order.iter().position(|&x| x == t).unwrap();
    // Reverse topological: the shared ancestor tx_a comes after both of its spenders.
    assert!(pos(txid_a) > pos(txid_b));
    assert!(pos(txid_a) > pos(txid_c));

    let acc_of = |t: Txid| {
        result
            .iter()
            .find(|(_, ct)| ct.txid == t)
            .unwrap()
            .0
            .clone()
    };
    // The accumulator flows from the root up into ancestors: tx_d (the root) spends tx_b and tx_c,
    // so its contribution is merged into each of them (alongside their own).
    assert_eq!(
        acc_of(txid_b),
        [txid_b, txid_d].into_iter().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        acc_of(txid_c),
        [txid_c, txid_d].into_iter().collect::<BTreeSet<_>>()
    );
    // tx_a is reached from both tx_b and tx_c, so the full descendant-closure merges into it.
    assert_eq!(
        acc_of(txid_a),
        [txid_a, txid_b, txid_c, txid_d]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    // tx_d is a root and is never yielded.
    assert!(result.iter().all(|(_, ct)| ct.txid != txid_d));
}

#[test]
fn ancestors_exact_size() {
    let (chain, tx_graph, [.., tx_d]) = diamond();
    let txid_d = tx_d.compute_txid();
    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let it = view.ancestors([txid_d], collect_self, |_ctx| true);
    assert_eq!(it.len(), 3);
    assert_eq!(it.count(), 3);
}

#[test]
fn ancestors_pruning_stops_a_branch() {
    let (chain, tx_graph, [tx_a, tx_b, tx_c, tx_d]) = diamond();
    let (txid_a, txid_b, txid_c, txid_d) = (
        tx_a.compute_txid(),
        tx_b.compute_txid(),
        tx_c.compute_txid(),
        tx_d.compute_txid(),
    );

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    // Prune tx_b: tx_a is no longer reached *through* tx_b, only through tx_c.
    let result: Vec<_> = view
        .ancestors([txid_d], collect_self, |ctx| ctx.txid != txid_b)
        .collect();

    // tx_b is still emitted (it is a direct ancestor of the root); only its onward edge is pruned.
    let order: Vec<Txid> = result.iter().map(|(_, ct)| ct.txid).collect();
    assert_eq!(order.len(), 3);

    let acc_of = |t: Txid| {
        result
            .iter()
            .find(|(_, ct)| ct.txid == t)
            .unwrap()
            .0
            .clone()
    };
    // tx_d still spends tx_b, so it merges into tx_b regardless of pruning tx_b's onward walk.
    assert_eq!(
        acc_of(txid_b),
        [txid_b, txid_d].into_iter().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        acc_of(txid_c),
        [txid_c, txid_d].into_iter().collect::<BTreeSet<_>>()
    );
    // The tx_b -> tx_a edge is pruned, so only tx_c's closure reaches tx_a (tx_b absent).
    assert_eq!(
        acc_of(txid_a),
        [txid_a, txid_c, txid_d]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
}
