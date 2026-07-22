#![cfg(feature = "miniscript")]

use std::collections::BTreeMap;

use bdk_chain::{
    local_chain::LocalChain, BlockId, ChainPosition, ConfirmationBlockTime, Eligibility, Trust,
    TxGraph,
};
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

    let parent = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("root"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
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

    // Create a non-coinbase transaction spending our confirmed parent.
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(parent_txid, 0),
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

    // A deeply-confirmed parent (settled for every threshold tested) so each tx has a known,
    // trusted ancestry, keeping the test focused on the confirmation threshold.
    let parent = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("root"), 0),
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(30_000),
                script_pubkey: ScriptBuf::new(),
            },
        ],
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

    // Transaction 0: anchored at height 5, has 11 confirmations (tip-5+1 = 15-5+1 = 11)
    let tx0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(parent_txid, 0),
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
            previous_output: OutPoint::new(parent_txid, 1),
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
            previous_output: OutPoint::new(parent_txid, 2),
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
    // unconfirmed output as settled rather than dropping it, even when it is tainted.
    let balance = view.balance([OutPoint::new(txid, 0)], |_| true, |_| true);
    assert_eq!(balance.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance.immature, Amount::ZERO);
    assert_eq!(balance.trusted_pending, Amount::ZERO);
    assert_eq!(balance.untrusted_pending, Amount::ZERO);
}

/// Taint must not cross the settled boundary: a settled (mined) ancestor that `does_taint` would
/// flag does not taint its unsettled descendants, because the walk stops at settled transactions
/// and never taints them.
#[test]
fn test_balance_taint_stops_at_settled_ancestor() {
    use std::collections::HashSet;

    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("g")), (1, hash!("b1")), (2, hash!("tip"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();
    let owned_spk = ScriptBuf::new();

    // A *settled* (confirmed) tx that itself spends a third-party coin — `does_taint` would flag
    // it.
    let settled_foreign = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("third_party"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(0)
    };
    let settled_foreign_txid = settled_foreign.compute_txid();

    // Unconfirmed child spending our own (settled) output.
    let child = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(settled_foreign_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(45_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(1)
    };
    let child_txid = child.compute_txid();

    let _ = tx_graph.insert_tx(settled_foreign.clone());
    let _ = tx_graph.insert_anchor(
        settled_foreign_txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );
    let _ = tx_graph.insert_tx(child.clone());
    let _ = tx_graph.insert_seen_at(child_txid, 1000);

    let owned = [
        OutPoint::new(settled_foreign_txid, 0),
        OutPoint::new(child_txid, 0),
    ]
    .into_iter()
    .collect::<HashSet<_>>();

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    // `child` is the only UTXO (`settled_foreign:0` is spent by it).
    let by_op = view
        .classify_outpoints(
            [OutPoint::new(child_txid, 0)],
            |c_tx| {
                c_tx.tx
                    .input
                    .iter()
                    .any(|txin| !owned.contains(&txin.previous_output))
            },
            |pos| pos.is_confirmed(),
        )
        .map(|(txout, eligibility)| (txout.outpoint, eligibility))
        .collect::<std::collections::HashMap<_, _>>();

    // The foreign-spending ancestor is settled, so the walk stops there and never taints `child`.
    assert_eq!(
        by_op[&OutPoint::new(child_txid, 0)],
        Eligibility::Unsettled(Trust::Trusted)
    );
}

/// `classify_outpoints` distinguishes an immature coinbase from a settled output.
#[test]
fn test_classify_immature_and_settled() {
    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("g")), (1, hash!("b1")), (2, hash!("tip"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();
    let spk = ScriptBuf::new();

    // Coinbase confirmed at height 1; far below `COINBASE_MATURITY` -> immature.
    let coinbase = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: spk.clone(),
        }],
        ..new_tx(0)
    };
    let coinbase_txid = coinbase.compute_txid();
    assert!(coinbase.is_coinbase());

    // A normal (non-coinbase) confirmed output.
    let normal = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("ext"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(30_000),
            script_pubkey: spk.clone(),
        }],
        ..new_tx(1)
    };
    let normal_txid = normal.compute_txid();

    for (txid, tx) in [(coinbase_txid, &coinbase), (normal_txid, &normal)] {
        let _ = tx_graph.insert_tx(tx.clone());
        let _ = tx_graph.insert_anchor(
            txid,
            ConfirmationBlockTime {
                block_id: chain.get(1).unwrap().block_id(),
                confirmation_time: 100,
            },
        );
    }

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let ops = [
        OutPoint::new(coinbase_txid, 0),
        OutPoint::new(normal_txid, 0),
    ];

    let by_op = view
        .classify_outpoints(ops, |_| false, |pos| pos.is_confirmed())
        .map(|(txout, eligibility)| (txout.outpoint, eligibility))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        by_op[&OutPoint::new(coinbase_txid, 0)],
        Eligibility::Immature
    );
    assert_eq!(by_op[&OutPoint::new(normal_txid, 0)], Eligibility::Settled);

    // The balance buckets reflect the same classification.
    let balance = view.balance(ops, |_| false, |pos| pos.is_confirmed());
    assert_eq!(balance.immature, Amount::from_sat(50_000));
    assert_eq!(balance.confirmed, Amount::from_sat(30_000));
}

/// Two UTXOs that share one tainting ancestor are both untrusted. Because the taint cache is shared
/// across outpoints, that common ancestor is only walked once.
#[test]
fn test_balance_taint_shared_ancestor() {
    use std::collections::HashSet;

    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("g")), (1, hash!("tip"))].into_iter().collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();
    let owned_spk = ScriptBuf::new();

    // Unconfirmed, funded by a third party -> taints itself. Two outputs.
    let foreign = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("third_party"), 0),
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(30_000),
                script_pubkey: owned_spk.clone(),
            },
            TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: owned_spk.clone(),
            },
        ],
        ..new_tx(0)
    };
    let foreign_txid = foreign.compute_txid();

    // Two children, each spending one of `foreign`'s outputs, so both share the tainting ancestor.
    let child_a = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(foreign_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(29_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(1)
    };
    let child_a_txid = child_a.compute_txid();
    let child_b = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(foreign_txid, 1),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(19_000),
            script_pubkey: owned_spk.clone(),
        }],
        ..new_tx(2)
    };
    let child_b_txid = child_b.compute_txid();

    for tx in [&foreign, &child_a, &child_b] {
        let _ = tx_graph.insert_tx(tx.clone());
        let _ = tx_graph.insert_seen_at(tx.compute_txid(), 1000);
    }

    let owned = [
        OutPoint::new(foreign_txid, 0),
        OutPoint::new(foreign_txid, 1),
        OutPoint::new(child_a_txid, 0),
        OutPoint::new(child_b_txid, 0),
    ]
    .into_iter()
    .collect::<HashSet<_>>();

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    // Both children descend from the same tainting `foreign`, so both are untrusted.
    let balance = view.balance(
        [
            OutPoint::new(child_a_txid, 0),
            OutPoint::new(child_b_txid, 0),
        ],
        |c_tx| {
            c_tx.tx
                .input
                .iter()
                .any(|txin| !owned.contains(&txin.previous_output))
        },
        |pos| pos.is_confirmed(),
    );
    assert_eq!(balance.trusted_pending, Amount::ZERO);
    assert_eq!(balance.untrusted_pending, Amount::from_sat(29_000 + 19_000));
}

/// `classify_outpoints` skips outpoints that are already spent or absent from the view.
#[test]
fn test_classify_skips_spent_and_unknown() {
    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("g")), (1, hash!("tip"))].into_iter().collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();
    let spk = ScriptBuf::new();

    // `parent`'s output is spent by `child`.
    let parent = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("ext"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: spk.clone(),
        }],
        ..new_tx(0)
    };
    let parent_txid = parent.compute_txid();
    let child = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(parent_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(40_000),
            script_pubkey: spk.clone(),
        }],
        ..new_tx(1)
    };
    let child_txid = child.compute_txid();
    for tx in [&parent, &child] {
        let _ = tx_graph.insert_tx(tx.clone());
        let _ = tx_graph.insert_seen_at(tx.compute_txid(), 1000);
    }

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    let classified = view
        .classify_outpoints(
            [
                OutPoint::new(parent_txid, 0),      // spent by `child` -> skipped
                OutPoint::new(hash!("unknown"), 0), // not in the view -> skipped
                OutPoint::new(child_txid, 0),       // the only real UTXO
            ],
            |_| false,
            |pos| pos.is_confirmed(),
        )
        .collect::<Vec<_>>();

    assert_eq!(classified.len(), 1);
    assert_eq!(classified[0].0.outpoint, OutPoint::new(child_txid, 0));
}

/// An immature coinbase stays `Immature` even when `is_settled` treats it as unsettled.
#[test]
fn test_immature_coinbase_stays_immature_when_unsettled() {
    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("g")), (1, hash!("tip"))].into_iter().collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    let coinbase = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(0)
    };
    let txid = coinbase.compute_txid();
    let _ = tx_graph.insert_tx(coinbase);
    let _ = tx_graph.insert_anchor(
        txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = view.tip().height;

    // Require 3 confirmations to be settled.
    let by_op = view
        .classify_outpoints([OutPoint::new(txid, 0)], |_| false, settled(tip_height, 3))
        .map(|(txout, e)| (txout.outpoint, e))
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(by_op[&OutPoint::new(txid, 0)], Eligibility::Immature);
}

/// Check maturity and settledness are independent axes.
#[test]
fn test_mature_coinbase_is_settled_not_immature() {
    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("g")), (1, hash!("b1")), (100, hash!("tip"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    let coinbase = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(0)
    };
    let txid = coinbase.compute_txid();
    let _ = tx_graph.insert_tx(coinbase);
    let _ = tx_graph.insert_anchor(
        txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
    let tip_height = view.tip().height;

    let by_op = view
        .classify_outpoints([OutPoint::new(txid, 0)], |_| false, settled(tip_height, 3))
        .map(|(txout, e)| (txout.outpoint, e))
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(by_op[&OutPoint::new(txid, 0)], Eligibility::Settled);
}

/// An unconfirmed chain whose root parent tx is missing from the Canonical set is
/// `Unsettled(Unknown)`.
#[test]
fn test_unsettled_unknown_when_parent_root_missing() {
    let blocks: BTreeMap<u32, BlockHash> =
        [(0, hash!("g")), (1, hash!("tip"))].into_iter().collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    let child1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("missing_root"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let child1_txid = child1.compute_txid();

    let child2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(child1_txid, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(2)
    };
    let child2_txid = child2.compute_txid();

    for tx in [&child1, &child2] {
        let _ = tx_graph.insert_tx(tx.clone());
        let _ = tx_graph.insert_seen_at(tx.compute_txid(), 1000);
    }
    // `missing_root` is not inserted.

    let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());

    let by_op = view
        .classify_outpoints(
            [OutPoint::new(child2_txid, 0)],
            |_| false,
            |pos| pos.is_confirmed(),
        )
        .map(|(txout, e)| (txout.outpoint, e))
        .collect::<std::collections::HashMap<_, _>>();

    assert_eq!(
        by_op[&OutPoint::new(child2_txid, 0)],
        Eligibility::Unsettled(Trust::Unknown)
    );
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
