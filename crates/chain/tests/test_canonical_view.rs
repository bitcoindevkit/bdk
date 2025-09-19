#![cfg(feature = "miniscript")]

use std::collections::{BTreeMap, HashMap};

use bdk_chain::{local_chain::LocalChain, CanonicalizationParams, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};

// =============================================================================
// Test case pattern for extract_subgraph tests
// =============================================================================

#[derive(Clone, Copy)]
struct TestTx {
    /// Name to identify this tx
    name: &'static str,
    /// Which tx outputs this tx spends: &[(parent_name, vout)]
    spends: &'static [(&'static str, u32)],
    /// Number of outputs to create
    num_outputs: u32,
    /// Anchor height (None = unconfirmed, use seen_at)
    anchor_height: Option<u32>,
}

struct ExtractSubgraphTestCase {
    /// Transactions in the graph
    txs: &'static [TestTx],
    /// Names of transactions to extract
    extract: &'static [&'static str],
    /// Expected tx names in extracted view
    expect_extracted: &'static [&'static str],
    /// Expected tx names remaining in original view
    expect_remaining: &'static [&'static str],
}

fn build_chain(max_height: u32) -> LocalChain {
    use bitcoin::hashes::Hash;
    let blocks: BTreeMap<u32, BlockHash> = (0..=max_height)
        .map(|h| (h, BlockHash::hash(format!("block{}", h).as_bytes())))
        .collect();
    LocalChain::from_blocks(blocks).unwrap()
}

fn build_test_graph(
    chain: &LocalChain,
    txs: &[TestTx],
) -> (TxGraph<ConfirmationBlockTime>, HashMap<&'static str, Txid>) {
    let mut tx_graph = TxGraph::default();
    let mut name_to_txid = HashMap::new();

    for (i, test_tx) in txs.iter().enumerate() {
        let inputs: Vec<TxIn> = test_tx
            .spends
            .iter()
            .map(|(parent, vout)| TxIn {
                previous_output: OutPoint::new(name_to_txid[parent], *vout),
                ..Default::default()
            })
            .collect();

        let tx = Transaction {
            input: inputs,
            output: (0..test_tx.num_outputs)
                .map(|_| TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: ScriptBuf::new(),
                })
                .collect(),
            ..new_tx(i as u32)
        };

        let txid = tx.compute_txid();
        name_to_txid.insert(test_tx.name, txid);
        let _ = tx_graph.insert_tx(tx);

        match test_tx.anchor_height {
            Some(h) => {
                let _ = tx_graph.insert_anchor(
                    txid,
                    ConfirmationBlockTime {
                        block_id: chain.get(h).unwrap().block_id(),
                        confirmation_time: h as u64 * 100,
                    },
                );
            }
            None => {
                let _ = tx_graph.insert_seen_at(txid, 1000);
            }
        }
    }

    (tx_graph, name_to_txid)
}

fn run_extract_subgraph_test(case: &ExtractSubgraphTestCase) {
    let max_height = case
        .txs
        .iter()
        .filter_map(|tx| tx.anchor_height)
        .max()
        .unwrap_or(0);
    let chain = build_chain(max_height + 1);

    let (tx_graph, name_to_txid) = build_test_graph(&chain, case.txs);
    let mut view = tx_graph.canonical_view(&chain, chain.tip().block_id(), Default::default());

    // Extract
    let extract_txids = case
        .extract
        .iter()
        .filter_map(|name| name_to_txid.get(name).copied());
    let extracted = view.extract_descendant_subgraph(extract_txids);

    // Verify extracted
    for name in case.expect_extracted {
        assert!(
            extracted.tx(name_to_txid[name]).is_some(),
            "{} should be in extracted",
            name
        );
    }

    // Verify not in extracted (should be remaining)
    for name in case.expect_remaining {
        assert!(
            extracted.tx(name_to_txid[name]).is_none(),
            "{} should not be in extracted",
            name
        );
    }

    // Verify remaining
    for name in case.expect_remaining {
        assert!(
            view.tx(name_to_txid[name]).is_some(),
            "{} should remain",
            name
        );
    }

    // Verify not remaining (should be extracted)
    for name in case.expect_extracted {
        assert!(
            view.tx(name_to_txid[name]).is_none(),
            "{} should not remain",
            name
        );
    }

    verify_spends_consistency(&extracted);
    verify_spends_consistency(&view);
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

    let chain_tip = chain.tip().block_id();
    let canonical_view =
        tx_graph.canonical_view(&chain, chain_tip, CanonicalizationParams::default());

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

    let canonical_view = tx_graph.canonical_view(
        &chain,
        chain.tip().block_id(),
        CanonicalizationParams::default(),
    );

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

    let canonical_view = tx_graph.canonical_view(
        &chain,
        chain.tip().block_id(),
        CanonicalizationParams::default(),
    );

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

#[test]
fn test_extract_subgraph_basic_chain() {
    // tx0 -> tx1 -> tx2
    run_extract_subgraph_test(&ExtractSubgraphTestCase {
        txs: &[
            TestTx {
                name: "tx0",
                spends: &[],
                num_outputs: 1,
                anchor_height: Some(1),
            },
            TestTx {
                name: "tx1",
                spends: &[("tx0", 0)],
                num_outputs: 1,
                anchor_height: Some(2),
            },
            TestTx {
                name: "tx2",
                spends: &[("tx1", 0)],
                num_outputs: 1,
                anchor_height: None,
            },
        ],
        extract: &["tx1"],
        expect_extracted: &["tx1", "tx2"],
        expect_remaining: &["tx0"],
    });
}

#[test]
fn test_extract_subgraph_diamond_graph() {
    //     tx0 (2 outputs)
    //    /    \
    //  tx1    tx2
    //    \    /
    //     tx3
    //      |
    //     tx4
    run_extract_subgraph_test(&ExtractSubgraphTestCase {
        txs: &[
            TestTx {
                name: "tx0",
                spends: &[],
                num_outputs: 2,
                anchor_height: Some(1),
            },
            TestTx {
                name: "tx1",
                spends: &[("tx0", 0)],
                num_outputs: 1,
                anchor_height: Some(2),
            },
            TestTx {
                name: "tx2",
                spends: &[("tx0", 1)],
                num_outputs: 1,
                anchor_height: Some(2),
            },
            TestTx {
                name: "tx3",
                spends: &[("tx1", 0), ("tx2", 0)],
                num_outputs: 1,
                anchor_height: Some(3),
            },
            TestTx {
                name: "tx4",
                spends: &[("tx3", 0)],
                num_outputs: 1,
                anchor_height: None,
            },
        ],
        extract: &["tx1", "tx2"],
        expect_extracted: &["tx1", "tx2", "tx3", "tx4"],
        expect_remaining: &["tx0"],
    });
}

#[test]
fn test_extract_subgraph_nonexistent_tx() {
    run_extract_subgraph_test(&ExtractSubgraphTestCase {
        txs: &[TestTx {
            name: "tx0",
            spends: &[],
            num_outputs: 1,
            anchor_height: Some(1),
        }],
        extract: &["fake"],
        expect_extracted: &[],
        expect_remaining: &["tx0"],
    });
}

#[test]
fn test_extract_subgraph_partial_chain() {
    // tx0 -> tx1 -> tx2 -> tx3
    run_extract_subgraph_test(&ExtractSubgraphTestCase {
        txs: &[
            TestTx {
                name: "tx0",
                spends: &[],
                num_outputs: 1,
                anchor_height: Some(1),
            },
            TestTx {
                name: "tx1",
                spends: &[("tx0", 0)],
                num_outputs: 1,
                anchor_height: Some(2),
            },
            TestTx {
                name: "tx2",
                spends: &[("tx1", 0)],
                num_outputs: 1,
                anchor_height: Some(3),
            },
            TestTx {
                name: "tx3",
                spends: &[("tx2", 0)],
                num_outputs: 1,
                anchor_height: Some(4),
            },
        ],
        extract: &["tx1"],
        expect_extracted: &["tx1", "tx2", "tx3"],
        expect_remaining: &["tx0"],
    });
}

// Helper function to verify spends field consistency
fn verify_spends_consistency<A: bdk_chain::Anchor>(view: &bdk_chain::CanonicalView<A>) {
    // Verify each transaction's outputs and their spent status
    for tx in view.txs() {
        // Verify the transaction exists
        assert!(view.tx(tx.txid).is_some());

        // For each output, check if it's properly tracked as spent
        for vout in 0..tx.tx.output.len() {
            let op = OutPoint::new(tx.txid, vout as u32);
            if let Some(txout) = view.txout(op) {
                // If this output is spent, verify the spending tx exists
                if let Some((_, spending_txid)) = txout.spent_by {
                    assert!(
                        view.tx(spending_txid).is_some(),
                        "Spending tx {spending_txid} not found in view"
                    );

                    // Verify the spending tx actually has this input
                    let spending_tx = view.tx(spending_txid).unwrap();
                    assert!(
                        spending_tx
                            .tx
                            .input
                            .iter()
                            .any(|input| input.previous_output == op),
                        "Transaction {spending_txid} doesn't actually spend outpoint {op}"
                    );
                }
            }
        }

        // For each input (except coinbase), verify it references valid outpoints
        if !tx.tx.is_coinbase() {
            for input in &tx.tx.input {
                // If the parent tx is in this view, verify the output exists and shows as spent
                if let Some(parent_txout) = view.txout(input.previous_output) {
                    assert_eq!(
                        parent_txout.spent_by.as_ref().map(|(_, txid)| *txid),
                        Some(tx.txid),
                        "Output {:?} should be marked as spent by tx {}",
                        input.previous_output,
                        tx.txid
                    );
                }
            }
        }
    }
}
