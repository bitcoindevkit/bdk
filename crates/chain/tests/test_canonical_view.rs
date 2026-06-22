#![cfg(feature = "miniscript")]

use std::collections::BTreeMap;

use bdk_chain::{local_chain::LocalChain, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut};

/// Builds an `is_settled` predicate requiring at least `min_confirmations` confirmations.
fn settled<A: bdk_chain::Anchor>(
    tip_height: u32,
    min_confirmations: u32,
) -> impl Fn(&bdk_chain::ChainPosition<A>) -> bool {
    move |pos| {
        pos.confirmation_height_upper_bound().is_some_and(|h| {
            tip_height.saturating_sub(h).saturating_add(1) >= min_confirmations.max(1)
        })
    }
}

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

    let canonical_view =
        chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = canonical_view.tip().height;

    // Test min_confirmations = 1: Should be confirmed (has 6 confirmations)
    let balance_1_conf = canonical_view.balance(
        [outpoint],
        |_| false, // taint nothing
        settled(tip_height, 1),
    );

    assert_eq!(balance_1_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_1_conf.trusted_pending, Amount::ZERO);

    // Test min_confirmations = 6: Should be confirmed (has exactly 6 confirmations)
    let balance_6_conf = canonical_view.balance(
        [outpoint],
        |_| false, // taint nothing
        settled(tip_height, 6),
    );
    assert_eq!(balance_6_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_6_conf.trusted_pending, Amount::ZERO);

    // Test min_confirmations = 7: Should be trusted pending (only has 6 confirmations)
    let balance_7_conf = canonical_view.balance(
        [outpoint],
        |_| false, // taint nothing
        settled(tip_height, 7),
    );
    assert_eq!(balance_7_conf.confirmed, Amount::ZERO);
    assert_eq!(balance_7_conf.trusted_pending, Amount::from_sat(50_000));

    // Test min_confirmations = 0: Should behave same as 1 (confirmed)
    let balance_0_conf = canonical_view.balance(
        [outpoint],
        |_| false, // taint nothing
        settled(tip_height, 0),
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

    let canonical_view =
        chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = canonical_view.tip().height;

    // Test with min_confirmations = 5. The output has only 3 confirmations, so it is unsettled and
    // treated as pending. Tainting everything demotes the pending output to untrusted.
    let balance = canonical_view.balance(
        [outpoint],
        |_| true, // taint everything
        settled(tip_height, 5),
    );

    // Should be untrusted pending (not deep enough to be settled, and tainted)
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
    let balance = canonical_view.balance(outpoints.clone(), |_| false, settled(tip_height, 5));

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
    let balance_high = canonical_view.balance(outpoints, |_| false, settled(tip_height, 10));

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

/// A pending output is `untrusted_pending` if it, or any of its unsettled ancestors, taints; the
/// taint propagates from a foreign-funded unconfirmed ancestor down to its descendants.
#[test]
fn test_balance_taint_propagates_through_unconfirmed_ancestry() {
    use std::collections::HashSet;

    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("g")), (1, hash!("b1")), (2, hash!("tip"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    let owned_spk = ScriptBuf::new();

    // A confirmed coin we own.
    let coin = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("coinbase"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(0)
    };
    let coin_txid = coin.compute_txid();

    // Unconfirmed, spends our own confirmed coin -> not tainted -> trusted_pending.
    let trusted = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(coin_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(40_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(1)
    };
    let trusted_txid = trusted.compute_txid();

    // Unconfirmed, funded by a third party (spends a foreign outpoint) -> taints itself.
    let foreign = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("third_party"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(30_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(2)
    };
    let foreign_txid = foreign.compute_txid();

    // Unconfirmed, spends our own `foreign` output -> tainted via its ancestor `foreign`.
    let chained = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(foreign_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(25_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(3)
    };
    let chained_txid = chained.compute_txid();

    let _ = tx_graph.insert_tx(coin.clone());
    let _ = tx_graph.insert_anchor(
        coin_txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );
    for tx in [&trusted, &foreign, &chained] {
        let _ = tx_graph.insert_tx(tx.clone());
        let _ = tx_graph.insert_seen_at(tx.compute_txid(), 1000);
    }

    // The set of outpoints we own.
    let owned = [
        OutPoint::new(coin_txid, 0),
        OutPoint::new(trusted_txid, 0),
        OutPoint::new(foreign_txid, 0),
        OutPoint::new(chained_txid, 0),
    ]
    .into_iter()
    .collect::<HashSet<_>>();

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    // Our unspent owned outputs: `trusted` and `chained` (the others are spent).
    let utxos = [
        OutPoint::new(trusted_txid, 0),
        OutPoint::new(chained_txid, 0),
    ];

    let balance = view.balance(
        utxos,
        // Taint any transaction that spends an outpoint we do not own.
        |c_tx| {
            c_tx.tx
                .input
                .iter()
                .any(|txin| !owned.contains(&txin.previous_output))
        },
        |pos| pos.is_confirmed(),
    );

    assert_eq!(balance.confirmed, Amount::ZERO);
    assert_eq!(balance.immature, Amount::ZERO);
    // `trusted` spends only our own (confirmed) coin -> trusted.
    assert_eq!(balance.trusted_pending, Amount::from_sat(40_000));
    // `chained` inherits taint from its foreign-funded ancestor `foreign`.
    assert_eq!(balance.untrusted_pending, Amount::from_sat(25_000));
}

/// `is_settled` is the sole authority on the settled boundary: a caller may treat an unconfirmed
/// output as settled, and its value must be counted (as settled), never silently dropped.
#[test]
fn test_balance_is_settled_is_authoritative_for_unconfirmed() {
    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("g")), (1, hash!("tip"))].into_iter().collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

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
    let _ = tx_graph.insert_seen_at(txid, 1000);

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    // An `is_settled` that claims everything is settled counts the (mature, non-coinbase)
    // unconfirmed output as settled rather than dropping it.
    let balance = view.balance([OutPoint::new(txid, 0)], |_| false, |_| true);
    assert_eq!(balance.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance.immature, Amount::ZERO);
    assert_eq!(balance.trusted_pending, Amount::ZERO);
    assert_eq!(balance.untrusted_pending, Amount::ZERO);
}
