#![cfg(feature = "miniscript")]

use std::collections::BTreeMap;

use bdk_chain::{local_chain::LocalChain, BlockId, ChainPosition, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut};

#[test]
fn test_min_confirmations_parameter() {
    // Create a local chain with several blocks
    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("block0")),
        (1, hash!("block1")),
        (2, hash!("block2")),
        (3, hash!("block3")),
        (4, hash!("block4")),
        (5, hash!("block5")),
        (6, hash!("block6")),
        (7, hash!("block7")),
        (8, hash!("block8")),
        (9, hash!("block9")),
        (10, hash!("block10")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();

    let mut tx_graph = TxGraph::default();

    // Create a non-coinbase transaction
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid = tx.compute_txid();
    let outpoint = OutPoint::new(txid, 0);

    // Insert transaction into graph
    let _ = tx_graph.insert_tx(tx.clone());

    // Test 1: Transaction confirmed at height 5, tip at height 10 (6 confirmations)
    let anchor_height_5 = ConfirmationBlockTime {
        block_id: chain.get(5).unwrap().block_id(),
        confirmation_time: 123456,
    };
    let _ = tx_graph.insert_anchor(txid, anchor_height_5);

    let canonical_view = chain.canonicalize(&tx_graph, chain.tip().block_id(), Default::default());

    // Test min_confirmations = 1: Should be confirmed (has 6 confirmations)
    let balance_1_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        1,
    );

    assert_eq!(balance_1_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_1_conf.trusted_pending, Amount::ZERO);

    // Test min_confirmations = 6: Should be confirmed (has exactly 6 confirmations)
    let balance_6_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        6,
    );
    assert_eq!(balance_6_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_6_conf.trusted_pending, Amount::ZERO);

    // Test min_confirmations = 7: Should be trusted pending (only has 6 confirmations)
    let balance_7_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        7,
    );
    assert_eq!(balance_7_conf.confirmed, Amount::ZERO);
    assert_eq!(balance_7_conf.trusted_pending, Amount::from_sat(50_000));

    // Test min_confirmations = 0: Should behave same as 1 (confirmed)
    let balance_0_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        0,
    );
    assert_eq!(balance_0_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_0_conf.trusted_pending, Amount::ZERO);
    assert_eq!(balance_0_conf, balance_1_conf);
}

#[test]
fn test_min_confirmations_with_untrusted_tx() {
    // Create a local chain
    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("genesis")),
        (1, hash!("b1")),
        (2, hash!("b2")),
        (3, hash!("b3")),
        (4, hash!("b4")),
        (5, hash!("b5")),
        (6, hash!("b6")),
        (7, hash!("b7")),
        (8, hash!("b8")),
        (9, hash!("b9")),
        (10, hash!("tip")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();

    let mut tx_graph = TxGraph::default();

    // Create a transaction
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(25_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid = tx.compute_txid();
    let outpoint = OutPoint::new(txid, 0);

    let _ = tx_graph.insert_tx(tx.clone());

    // Anchor at height 8, tip at height 10 (3 confirmations)
    let anchor = ConfirmationBlockTime {
        block_id: chain.get(8).unwrap().block_id(),
        confirmation_time: 123456,
    };
    let _ = tx_graph.insert_anchor(txid, anchor);

    let canonical_view = chain.canonicalize(&tx_graph, chain.tip().block_id(), Default::default());

    // Test with min_confirmations = 5 and untrusted predicate
    let balance = canonical_view.balance(
        [((), outpoint)],
        |_, _| false, // don't trust
        5,
    );

    // Should be untrusted pending (not enough confirmations and not trusted)
    assert_eq!(balance.confirmed, Amount::ZERO);
    assert_eq!(balance.trusted_pending, Amount::ZERO);
    assert_eq!(balance.untrusted_pending, Amount::from_sat(25_000));
}

#[test]
fn test_min_confirmations_multiple_transactions() {
    // Create a local chain
    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("genesis")),
        (1, hash!("b1")),
        (2, hash!("b2")),
        (3, hash!("b3")),
        (4, hash!("b4")),
        (5, hash!("b5")),
        (6, hash!("b6")),
        (7, hash!("b7")),
        (8, hash!("b8")),
        (9, hash!("b9")),
        (10, hash!("b10")),
        (11, hash!("b11")),
        (12, hash!("b12")),
        (13, hash!("b13")),
        (14, hash!("b14")),
        (15, hash!("tip")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();

    let mut tx_graph = TxGraph::default();

    // Create multiple transactions at different heights
    let mut outpoints = vec![];

    // Transaction 0: anchored at height 5, has 11 confirmations (tip-5+1 = 15-5+1 = 11)
    let tx0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent0"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid0 = tx0.compute_txid();
    let outpoint0 = OutPoint::new(txid0, 0);
    let _ = tx_graph.insert_tx(tx0);
    let _ = tx_graph.insert_anchor(
        txid0,
        ConfirmationBlockTime {
            block_id: chain.get(5).unwrap().block_id(),
            confirmation_time: 123456,
        },
    );
    outpoints.push(((), outpoint0));

    // Transaction 1: anchored at height 10, has 6 confirmations (15-10+1 = 6)
    let tx1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent1"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(20_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(2)
    };
    let txid1 = tx1.compute_txid();
    let outpoint1 = OutPoint::new(txid1, 0);
    let _ = tx_graph.insert_tx(tx1);
    let _ = tx_graph.insert_anchor(
        txid1,
        ConfirmationBlockTime {
            block_id: chain.get(10).unwrap().block_id(),
            confirmation_time: 123457,
        },
    );
    outpoints.push(((), outpoint1));

    // Transaction 2: anchored at height 13, has 3 confirmations (15-13+1 = 3)
    let tx2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent2"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(30_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(3)
    };
    let txid2 = tx2.compute_txid();
    let outpoint2 = OutPoint::new(txid2, 0);
    let _ = tx_graph.insert_tx(tx2);
    let _ = tx_graph.insert_anchor(
        txid2,
        ConfirmationBlockTime {
            block_id: chain.get(13).unwrap().block_id(),
            confirmation_time: 123458,
        },
    );
    outpoints.push(((), outpoint2));

    let canonical_view = chain.canonicalize(&tx_graph, chain.tip().block_id(), Default::default());

    // Test with min_confirmations = 5
    // tx0: 11 confirmations -> confirmed
    // tx1: 6 confirmations -> confirmed
    // tx2: 3 confirmations -> trusted pending
    let balance = canonical_view.balance(outpoints.clone(), |_, _| true, 5);

    assert_eq!(
        balance.confirmed,
        Amount::from_sat(10_000 + 20_000) // tx0 + tx1
    );
    assert_eq!(
        balance.trusted_pending,
        Amount::from_sat(30_000) // tx2
    );
    assert_eq!(balance.untrusted_pending, Amount::ZERO);

    // Test with min_confirmations = 10
    // tx0: 11 confirmations -> confirmed
    // tx1: 6 confirmations -> trusted pending
    // tx2: 3 confirmations -> trusted pending
    let balance_high = canonical_view.balance(outpoints, |_, _| true, 10);

    assert_eq!(
        balance_high.confirmed,
        Amount::from_sat(10_000) // only tx0
    );
    assert_eq!(
        balance_high.trusted_pending,
        Amount::from_sat(20_000 + 30_000) // tx1 + tx2
    );
    assert_eq!(balance_high.untrusted_pending, Amount::ZERO);
}

/// Txs with stale anchor and that have `last_evicted >= last_seen` should be excluded from
/// canonicalization.
#[test]
fn test_evicted_stale_anchored_tx_not_canonical() {
    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("genesis")), (1, hash!("b1")), (2, hash!("tip"))]
            .into_iter()
            .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();

    let mut tx_graph = TxGraph::default();
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid = tx.compute_txid();
    let _ = tx_graph.insert_tx(tx);
    let _ = tx_graph.insert_anchor(
        txid,
        ConfirmationBlockTime {
            block_id: BlockId {
                height: 1,
                hash: hash!("stale"),
            },
            confirmation_time: 123456,
        },
    );
    let _ = tx_graph.insert_seen_at(txid, 100);
    let _ = tx_graph.insert_evicted_at(txid, 200);

    let view = chain.canonicalize(&tx_graph, chain.tip().block_id(), Default::default());
    assert!(
        !view.txs().any(|tx| tx.txid == txid),
        "evicted leftover tx must not be canonical"
    );
}

/// An evicted transaction swept transitively as the ancestor of a stale-anchored transaction must
/// still report the mempool sighting it genuinely has.
///
/// `ChainPosition::Unconfirmed::last_seen` is documented as `None` only when the transaction was
/// never seen in the mempool *and* only seen in a conflicting chain. An evicted transaction fails
/// the first condition — it has a real `last_seen` — but
/// `TxGraph::txids_by_descending_last_seen` filters evicted transactions out, so it never reaches
/// the mempool stage of canonicalization and instead arrives via `ObservedIn::Block`. Forcing
/// `None` there would report `first_seen: Some(_), last_seen: None`, i.e. "first seen at t, never
/// seen", since `first_seen` is passed through unconditionally.
#[test]
fn test_evicted_ancestor_of_stale_anchored_tx_keeps_last_seen() {
    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("genesis")), (1, hash!("b1")), (2, hash!("tip"))]
            .into_iter()
            .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();

    let mut tx_graph = TxGraph::default();

    // Parent: seen in the mempool at 100, then evicted at 200.
    let parent = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("grandparent"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let parent_txid = parent.compute_txid();
    let _ = tx_graph.insert_tx(parent.clone());
    let _ = tx_graph.insert_seen_at(parent_txid, 100);
    let _ = tx_graph.insert_evicted_at(parent_txid, 200);

    // Child: spends the parent and is anchored only in a block that is not in the best chain, so
    // it is canonicalized in the leftover stage as `ObservedIn::Block`. It has no mempool
    // sighting of its own, so it is not evicted and the leftover stage does not skip it.
    let child = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(parent_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(40_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(2)
    };
    let child_txid = child.compute_txid();
    let _ = tx_graph.insert_tx(child);
    let _ = tx_graph.insert_anchor(
        child_txid,
        ConfirmationBlockTime {
            block_id: BlockId {
                height: 1,
                hash: hash!("stale"),
            },
            confirmation_time: 123456,
        },
    );

    let view = chain.canonicalize(&tx_graph, chain.tip().block_id(), Default::default());

    let parent_pos = view
        .txs()
        .find(|c| c.txid == parent_txid)
        .expect("parent is swept canonical as an ancestor of the child")
        .pos;

    match parent_pos {
        ChainPosition::Unconfirmed {
            first_seen,
            last_seen,
        } => {
            assert_eq!(first_seen, Some(100), "first_seen is passed through");
            assert_eq!(
                last_seen,
                Some(100),
                "an evicted tx was seen in the mempool, so its last_seen must be reported"
            );
        }
        pos => panic!("parent must be unconfirmed, got {pos:?}"),
    }
}

/// A transitively-marked ancestor must not lose a later mempool sighting of its own.
///
/// `ObservedIn::Mempool` carries the *descendant's* `last_seen`, which is only an inference — a
/// child cannot sit in the mempool without its parent. `TxGraph::txids_by_descending_last_seen`
/// iterates newest-first and `mark_canonical` skips anything already canonical, so a sweep
/// normally only reaches an ancestor whose own sighting is older. Evicted transactions are
/// filtered out of that iteration, though, so an evicted ancestor can be swept with a timestamp
/// older than the one we directly observed for it.
#[test]
fn test_evicted_ancestor_keeps_its_own_later_last_seen() {
    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("genesis")), (1, hash!("tip"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();

    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    // Parent: directly observed in the mempool at 300, then evicted at 400. The eviction keeps
    // it out of the mempool stage, so it is only ever marked canonical transitively.
    let parent = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("grandparent"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let parent_txid = parent.compute_txid();
    let _ = tx_graph.insert_tx(parent);
    let _ = tx_graph.insert_seen_at(parent_txid, 300);
    let _ = tx_graph.insert_evicted_at(parent_txid, 400);

    // Child: spends the parent, seen at 200 — *earlier* than the parent's own sighting.
    let child = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(parent_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(40_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(2)
    };
    let child_txid = child.compute_txid();
    let _ = tx_graph.insert_tx(child);
    let _ = tx_graph.insert_seen_at(child_txid, 200);

    let view = chain.canonicalize(&tx_graph, chain.tip().block_id(), Default::default());

    let parent_pos = view
        .txs()
        .find(|c| c.txid == parent_txid)
        .expect("parent is swept canonical as an ancestor of the child")
        .pos;

    match parent_pos {
        ChainPosition::Unconfirmed {
            first_seen,
            last_seen,
        } => {
            assert_eq!(first_seen, Some(300));
            assert_eq!(
                last_seen,
                Some(300),
                "the parent's own sighting is later than the child's, so it must win; \
                 reporting the child's 200 would also put last_seen before first_seen"
            );
        }
        pos => panic!("parent must be unconfirmed, got {pos:?}"),
    }
}
