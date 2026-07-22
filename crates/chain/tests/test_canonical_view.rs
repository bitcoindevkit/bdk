#![cfg(feature = "miniscript")]

use std::collections::BTreeMap;

use bdk_chain::{local_chain::LocalChain, BlockId, ChainPosition, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut};

/// Builds an `is_settled` predicate requiring at least `min_confirmations` confirmations.
fn settled(
    tip_height: u32,
    min_confirmations: u32,
) -> impl Fn(&ChainPosition<ConfirmationBlockTime>) -> bool {
    let min_confirmations = min_confirmations.max(1); // 0 and 1 behave identically
    move |pos| {
        pos.confirmation_height_upper_bound()
            .is_some_and(|h| tip_height.saturating_sub(h).saturating_add(1) >= min_confirmations)
    }
}

#[test]
fn test_is_settled_boundary() {
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

    let canonical_view =
        chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = canonical_view.tip().height;

    // Test min_confirmations = 1: Should be confirmed (has 6 confirmations)
    let balance_1_conf = canonical_view.balance(
        [outpoint],
        |_tx| false, // leave trust to ancestry
        settled(tip_height, 1),
    );

    assert_eq!(balance_1_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_1_conf.trusted_pending, Amount::ZERO);

    // Test min_confirmations = 6: Should be confirmed (has exactly 6 confirmations)
    let balance_6_conf = canonical_view.balance(
        [outpoint],
        |_tx| false, // leave trust to ancestry
        settled(tip_height, 6),
    );
    assert_eq!(balance_6_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_6_conf.trusted_pending, Amount::ZERO);

    // Test min_confirmations = 7: Should be trusted pending (only has 6 confirmations)
    let balance_7_conf = canonical_view.balance(
        [outpoint],
        |_tx| false, // leave trust to ancestry
        settled(tip_height, 7),
    );
    assert_eq!(balance_7_conf.confirmed, Amount::ZERO);
    assert_eq!(balance_7_conf.trusted_pending, Amount::from_sat(50_000));
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

    // A settled parent, so ancestry alone would make the child trusted and `does_taint` is what
    // decides the outcome.
    let parent = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("root"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(25_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(0)
    };
    let parent_txid = parent.compute_txid();
    let _ = tx_graph.insert_tx(parent.clone());
    let _ = tx_graph.insert_anchor(
        parent_txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );

    // Create a transaction
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(parent_txid, 0),
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

    let canonical_view =
        chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = canonical_view.tip().height;

    // Test with min_confirmations = 5 and everything tainted
    let balance = canonical_view.balance(
        [outpoint],
        |_tx| true, // taint everything
        settled(tip_height, 5),
    );

    // Should be untrusted pending (not enough confirmations and not trusted)
    assert_eq!(balance.confirmed, Amount::ZERO);
    assert_eq!(balance.trusted_pending, Amount::ZERO);
    assert_eq!(balance.untrusted_pending, Amount::from_sat(25_000));

    // Without the taint, the settled ancestry makes it trusted instead.
    let balance = canonical_view.balance(
        [outpoint],
        |_tx| false, // leave trust to ancestry
        settled(tip_height, 5),
    );
    assert_eq!(balance.trusted_pending, Amount::from_sat(25_000));
    assert_eq!(balance.untrusted_pending, Amount::ZERO);
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
    outpoints.push(outpoint0);

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
    outpoints.push(outpoint1);

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
    outpoints.push(outpoint2);

    let canonical_view =
        chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = canonical_view.tip().height;

    // Test with min_confirmations = 5
    // tx0: 11 confirmations -> confirmed
    // tx1: 6 confirmations -> confirmed
    // tx2: 3 confirmations -> trusted pending
    let balance = canonical_view.balance(outpoints.clone(), |_tx| false, settled(tip_height, 5));

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
    let balance_high = canonical_view.balance(outpoints, |_tx| false, settled(tip_height, 10));

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

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    assert!(
        !view.txs().any(|tx| tx.txid == txid),
        "evicted leftover tx must not be canonical"
    );
}
