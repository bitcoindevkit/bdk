#![cfg(feature = "miniscript")]

use bdk_chain::{local_chain::LocalChain, CanonicalizationParams, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut};
use std::collections::BTreeMap;

#[test]
fn test_additional_confirmations_parameter() {
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

    // Test additional_confirmations = 0: Should be confirmed (has 6 confirmations, needs 1)
    let balance_1_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        0,
    );

    assert_eq!(balance_1_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_1_conf.trusted_pending, Amount::ZERO);

    // Test additional_confirmations = 5: Should be confirmed (has 6 confirmations, needs 6)
    let balance_6_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        5,
    );
    assert_eq!(balance_6_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_6_conf.trusted_pending, Amount::ZERO);

    // Test additional_confirmations = 6: Should be trusted pending (has 6 confirmations, needs 7)
    let balance_7_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        6,
    );
    assert_eq!(balance_7_conf.confirmed, Amount::ZERO);
    assert_eq!(balance_7_conf.trusted_pending, Amount::from_sat(50_000));

    // Test additional_confirmations = 0: Should be confirmed
    let balance_0_conf = canonical_view.balance(
        [((), outpoint)],
        |_, _| true, // trust all
        0,
    );
    assert_eq!(balance_0_conf.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance_0_conf.trusted_pending, Amount::ZERO);
}

#[test]
fn test_additional_confirmations_with_untrusted_tx() {
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

    // Test with additional_confirmations = 4 and untrusted predicate (requires 5 total)
    let balance = canonical_view.balance(
        [((), outpoint)],
        |_, _| false, // don't trust
        4,
    );

    // Should be untrusted pending (not enough confirmations and not trusted)
    assert_eq!(balance.confirmed, Amount::ZERO);
    assert_eq!(balance.trusted_pending, Amount::ZERO);
    assert_eq!(balance.untrusted_pending, Amount::from_sat(25_000));
}

#[test]
fn test_additional_confirmations_multiple_transactions() {
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

    // Test with additional_confirmations = 4 (requires 5 total)
    // tx0: 11 confirmations -> confirmed
    // tx1: 6 confirmations -> confirmed
    // tx2: 3 confirmations -> trusted pending
    let balance = canonical_view.balance(outpoints.clone(), |_, _| true, 4);

    assert_eq!(
        balance.confirmed,
        Amount::from_sat(10_000 + 20_000) // tx0 + tx1
    );
    assert_eq!(
        balance.trusted_pending,
        Amount::from_sat(30_000) // tx2
    );
    assert_eq!(balance.untrusted_pending, Amount::ZERO);

    // Test with additional_confirmations = 9 (requires 10 total)
    // tx0: 11 confirmations -> confirmed
    // tx1: 6 confirmations -> trusted pending
    // tx2: 3 confirmations -> trusted pending
    let balance_high = canonical_view.balance(outpoints, |_, _| true, 9);

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
    // Test extracting a simple chain: tx0 -> tx1 -> tx2
    // Extracting tx1 should also extract tx2 (its child)

    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("genesis")),
        (1, hash!("block1")),
        (2, hash!("block2")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::default();

    // Create tx0 (coinbase - will be canonical)
    let tx0 = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(0)
    };
    let txid0 = tx0.compute_txid();
    let _ = tx_graph.insert_tx(tx0.clone());
    let _ = tx_graph.insert_anchor(
        txid0,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );

    // Create tx1 that spends from tx0
    let tx1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(txid0, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(90_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid1 = tx1.compute_txid();
    let _ = tx_graph.insert_tx(tx1.clone());
    let _ = tx_graph.insert_anchor(
        txid1,
        ConfirmationBlockTime {
            block_id: chain.get(2).unwrap().block_id(),
            confirmation_time: 200,
        },
    );

    // Create tx2 that spends from tx1
    let tx2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(txid1, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(80_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(2)
    };
    let txid2 = tx2.compute_txid();
    let _ = tx_graph.insert_tx(tx2.clone());
    // tx2 is unconfirmed but seen
    let _ = tx_graph.insert_seen_at(txid2, 300);

    let mut canonical_view = tx_graph.canonical_view(
        &chain,
        chain.tip().block_id(),
        CanonicalizationParams::default(),
    );

    // Extract tx1 and its descendants
    let extracted = canonical_view.extract_subgraph([txid1].into_iter());

    // Verify extracted view contains tx1 and tx2 but not tx0
    assert!(extracted.tx(txid1).is_some());
    assert!(extracted.tx(txid2).is_some());
    assert!(extracted.tx(txid0).is_none());

    // Verify remaining view contains only tx0
    assert!(canonical_view.tx(txid0).is_some());
    assert!(canonical_view.tx(txid1).is_none());
    assert!(canonical_view.tx(txid2).is_none());

    // Verify that tx1's input (spending from tx0) is properly handled
    let tx1_data = extracted.tx(txid1).unwrap();
    assert_eq!(tx1_data.tx.input[0].previous_output.txid, txid0);

    // Verify that tx2's input (spending from tx1) is properly handled
    let tx2_data = extracted.tx(txid2).unwrap();
    assert_eq!(tx2_data.tx.input[0].previous_output.txid, txid1);

    // Verify through txout that spending relationships are maintained
    // tx0 output 0 is spent (but tx0 is not in the extracted view)
    assert!(extracted.txout(OutPoint::new(txid0, 0)).is_none());

    // tx1 output 0 is spent by tx2
    let tx1_out = extracted.txout(OutPoint::new(txid1, 0)).unwrap();
    assert_eq!(
        tx1_out.spent_by.as_ref().map(|(_, txid)| *txid),
        Some(txid2)
    );

    // tx2 output 0 is unspent
    let tx2_out = extracted.txout(OutPoint::new(txid2, 0)).unwrap();
    assert!(tx2_out.spent_by.is_none());

    // Verify remaining view: tx0 output should be unspent (tx1 was removed)
    let tx0_out = canonical_view.txout(OutPoint::new(txid0, 0)).unwrap();
    assert!(tx0_out.spent_by.is_none());
}

#[test]
fn test_extract_subgraph_complex_graph() {
    // Test a more complex graph:
    //       tx0
    //      /    \
    //    tx1    tx2
    //      \    /
    //       tx3
    //        |
    //       tx4

    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("genesis")),
        (1, hash!("block1")),
        (2, hash!("block2")),
        (3, hash!("block3")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::default();

    // Create tx0 with 2 outputs
    let tx0 = Transaction {
        output: vec![
            TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..new_tx(0)
    };
    let txid0 = tx0.compute_txid();
    let _ = tx_graph.insert_tx(tx0.clone());
    let _ = tx_graph.insert_anchor(
        txid0,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );

    // tx1 spends first output of tx0
    let tx1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(txid0, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(45_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid1 = tx1.compute_txid();
    let _ = tx_graph.insert_tx(tx1.clone());
    let _ = tx_graph.insert_anchor(
        txid1,
        ConfirmationBlockTime {
            block_id: chain.get(2).unwrap().block_id(),
            confirmation_time: 200,
        },
    );

    // tx2 spends second output of tx0
    let tx2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(txid0, 1),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(45_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(2)
    };
    let txid2 = tx2.compute_txid();
    let _ = tx_graph.insert_tx(tx2.clone());
    let _ = tx_graph.insert_anchor(
        txid2,
        ConfirmationBlockTime {
            block_id: chain.get(2).unwrap().block_id(),
            confirmation_time: 201,
        },
    );

    // tx3 spends from both tx1 and tx2
    let tx3 = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(txid1, 0),
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(txid2, 0),
                ..Default::default()
            },
        ],
        output: vec![TxOut {
            value: Amount::from_sat(85_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(3)
    };
    let txid3 = tx3.compute_txid();
    let _ = tx_graph.insert_tx(tx3.clone());
    let _ = tx_graph.insert_anchor(
        txid3,
        ConfirmationBlockTime {
            block_id: chain.get(3).unwrap().block_id(),
            confirmation_time: 300,
        },
    );

    // tx4 spends from tx3
    let tx4 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(txid3, 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(80_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(4)
    };
    let txid4 = tx4.compute_txid();
    let _ = tx_graph.insert_tx(tx4.clone());
    // tx4 is unconfirmed but seen
    let _ = tx_graph.insert_seen_at(txid4, 400);

    let mut canonical_view = tx_graph.canonical_view(
        &chain,
        chain.tip().block_id(),
        CanonicalizationParams::default(),
    );

    // Extract tx1 and tx2, should also get tx3 and tx4 as descendants
    let extracted = canonical_view.extract_subgraph([txid1, txid2].into_iter());

    // Verify extracted view contains tx1, tx2, tx3, tx4 but not tx0
    assert!(extracted.tx(txid1).is_some());
    assert!(extracted.tx(txid2).is_some());
    assert!(extracted.tx(txid3).is_some());
    assert!(extracted.tx(txid4).is_some());
    assert!(extracted.tx(txid0).is_none());

    // Verify remaining view contains only tx0
    assert!(canonical_view.tx(txid0).is_some());
    assert!(canonical_view.tx(txid1).is_none());
    assert!(canonical_view.tx(txid2).is_none());
    assert!(canonical_view.tx(txid3).is_none());
    assert!(canonical_view.tx(txid4).is_none());

    // Verify spends field correctness
    verify_spends_consistency(&extracted);
    verify_spends_consistency(&canonical_view);
}

#[test]
fn test_extract_subgraph_nonexistent_tx() {
    // Test extracting a transaction that doesn't exist
    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("genesis")), (1, hash!("block1"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::default();

    let tx = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(100_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(0)
    };
    let txid = tx.compute_txid();
    let _ = tx_graph.insert_tx(tx);
    let _ = tx_graph.insert_anchor(
        txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 100,
        },
    );

    let mut canonical_view = tx_graph.canonical_view(
        &chain,
        chain.tip().block_id(),
        CanonicalizationParams::default(),
    );

    // Try to extract a non-existent transaction
    let fake_txid = hash!("nonexistent");
    let extracted = canonical_view.extract_subgraph([fake_txid].into_iter());

    // Should return empty view
    assert_eq!(extracted.txs().count(), 0);

    // Original view should be unchanged
    assert!(canonical_view.tx(txid).is_some());
}

#[test]
fn test_extract_subgraph_partial_chain() {
    // Test extracting from the middle of a chain
    // tx0 -> tx1 -> tx2 -> tx3
    // Extract tx1 should give tx1, tx2, tx3 but not tx0

    let blocks: BTreeMap<u32, BlockHash> = [
        (0, hash!("genesis")),
        (1, hash!("block1")),
        (2, hash!("block2")),
        (3, hash!("block3")),
        (4, hash!("block4")),
    ]
    .into_iter()
    .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let mut tx_graph = TxGraph::default();

    // Build chain of transactions
    let mut txids = vec![];
    let mut prev_txid = None;

    for i in 0..4 {
        let tx = if let Some(prev) = prev_txid {
            Transaction {
                input: vec![TxIn {
                    previous_output: OutPoint::new(prev, 0),
                    ..Default::default()
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(100_000 - i * 10_000),
                    script_pubkey: ScriptBuf::new(),
                }],
                ..new_tx(i as u32)
            }
        } else {
            Transaction {
                output: vec![TxOut {
                    value: Amount::from_sat(100_000),
                    script_pubkey: ScriptBuf::new(),
                }],
                ..new_tx(i as u32)
            }
        };

        let txid = tx.compute_txid();
        let _ = tx_graph.insert_tx(tx);

        // Anchor each transaction in successive blocks
        let _ = tx_graph.insert_anchor(
            txid,
            ConfirmationBlockTime {
                block_id: chain.get(i as u32 + 1).unwrap().block_id(),
                confirmation_time: (i + 1) * 100,
            },
        );

        txids.push(txid);
        prev_txid = Some(txid);
    }

    let mut canonical_view = tx_graph.canonical_view(
        &chain,
        chain.tip().block_id(),
        CanonicalizationParams::default(),
    );

    // Extract from tx1
    let extracted = canonical_view.extract_subgraph([txids[1]].into_iter());

    // Verify extracted contains tx1, tx2, tx3 but not tx0
    assert!(extracted.tx(txids[1]).is_some());
    assert!(extracted.tx(txids[2]).is_some());
    assert!(extracted.tx(txids[3]).is_some());
    assert!(extracted.tx(txids[0]).is_none());

    // Verify remaining contains only tx0
    assert!(canonical_view.tx(txids[0]).is_some());
    assert!(canonical_view.tx(txids[1]).is_none());

    verify_spends_consistency(&extracted);
    verify_spends_consistency(&canonical_view);
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
