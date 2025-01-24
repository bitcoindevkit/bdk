#![cfg(feature = "miniscript")]

#[macro_use]
mod common;
use bdk_chain::{collections::*, BlockId, ConfirmationBlockTime};
use bdk_chain::{
    local_chain::LocalChain,
    tx_graph::{self, CalculateFeeError},
    tx_graph::{ChangeSet, TxGraph},
    Anchor, ChainOracle, ChainPosition, Merge,
};
use bdk_testenv::{block_id, hash, utils::new_tx};
use bitcoin::{
    absolute, hashes::Hash, transaction, Amount, BlockHash, OutPoint, ScriptBuf, SignedAmount,
    Transaction, TxIn, TxOut, Txid,
};
use common::*;
use core::iter;
use rand::RngCore;
use std::sync::Arc;
use std::vec;

#[test]
fn insert_txouts() {
    // 2 (Outpoint, TxOut) tuples that denotes original data in the graph, as partial transactions.
    let original_ops = [
        (
            OutPoint::new(hash!("tx1"), 1),
            TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
        ),
        (
            OutPoint::new(hash!("tx1"), 2),
            TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: ScriptBuf::new(),
            },
        ),
    ];

    // Another (OutPoint, TxOut) tuple to be used as update as partial transaction.
    let update_ops = [(
        OutPoint::new(hash!("tx2"), 0),
        TxOut {
            value: Amount::from_sat(20_000),
            script_pubkey: ScriptBuf::new(),
        },
    )];

    // One full transaction to be included in the update
    let update_tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(30_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };

    // Conf anchor used to mark the full transaction as confirmed.
    let conf_anchor = BlockId {
        height: 100,
        hash: hash!("random blockhash"),
    };

    // Unconfirmed seen_at timestamp to mark the partial transactions as unconfirmed.
    let unconf_seen_at = 1000000_u64;

    // Make the original graph
    let mut graph = {
        let mut graph = TxGraph::<BlockId>::default();
        for (outpoint, txout) in &original_ops {
            assert_eq!(
                graph.insert_txout(*outpoint, txout.clone()),
                ChangeSet {
                    txouts: [(*outpoint, txout.clone())].into(),
                    ..Default::default()
                }
            );
        }
        graph
    };

    // Make the update graph
    let update = {
        let mut update = tx_graph::TxUpdate::default();
        for (outpoint, txout) in &update_ops {
            // Insert partials transactions.
            update.txouts.insert(*outpoint, txout.clone());
            // Mark them unconfirmed.
            update.seen_ats.insert(outpoint.txid, unconf_seen_at);
        }

        // Insert the full transaction.
        update.txs.push(update_tx.clone().into());
        // Mark it as confirmed.
        update
            .anchors
            .insert((conf_anchor, update_tx.compute_txid()));
        update
    };

    // Check the resulting addition.
    let changeset = graph.apply_update(update);

    assert_eq!(
        changeset,
        ChangeSet {
            txs: [Arc::new(update_tx.clone())].into(),
            txouts: update_ops.clone().into(),
            anchors: [(conf_anchor, update_tx.compute_txid()),].into(),
            last_seen: [(hash!("tx2"), 1000000)].into()
        }
    );

    // Apply changeset and check the new graph counts.
    graph.apply_changeset(changeset);
    assert_eq!(graph.all_txouts().count(), 4);
    assert_eq!(graph.full_txs().count(), 1);
    assert_eq!(graph.floating_txouts().count(), 3);

    // Check TxOuts are fetched correctly from the graph.
    assert_eq!(
        graph.tx_outputs(hash!("tx1")).expect("should exists"),
        [
            (
                1u32,
                &TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: ScriptBuf::new(),
                }
            ),
            (
                2u32,
                &TxOut {
                    value: Amount::from_sat(20_000),
                    script_pubkey: ScriptBuf::new(),
                }
            )
        ]
        .into()
    );

    assert_eq!(
        graph
            .tx_outputs(update_tx.compute_txid())
            .expect("should exists"),
        [(
            0u32,
            &TxOut {
                value: Amount::from_sat(30_000),
                script_pubkey: ScriptBuf::new()
            }
        )]
        .into()
    );

    // Check that the initial_changeset is correct
    assert_eq!(
        graph.initial_changeset(),
        ChangeSet {
            txs: [Arc::new(update_tx.clone())].into(),
            txouts: update_ops.into_iter().chain(original_ops).collect(),
            anchors: [(conf_anchor, update_tx.compute_txid()),].into(),
            last_seen: [(hash!("tx2"), 1000000)].into()
        }
    );
}

#[test]
fn insert_tx_graph_doesnt_count_coinbase_as_spent() {
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![],
    };

    let mut graph = TxGraph::<ConfirmationBlockTime>::default();
    let changeset = graph.insert_tx(tx);
    assert!(!changeset.is_empty());
    assert!(graph.outspends(OutPoint::null()).is_empty());
    assert!(graph.tx_spends(Txid::all_zeros()).next().is_none());
}

#[test]
fn insert_tx_graph_keeps_track_of_spend() {
    let tx1 = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut::NULL],
    };

    let op = OutPoint {
        txid: tx1.compute_txid(),
        vout: 0,
    };

    let tx2 = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: op,
            ..Default::default()
        }],
        output: vec![],
    };

    let mut graph1 = TxGraph::<ConfirmationBlockTime>::default();
    let mut graph2 = TxGraph::<ConfirmationBlockTime>::default();

    // insert in different order
    let _ = graph1.insert_tx(tx1.clone());
    let _ = graph1.insert_tx(tx2.clone());

    let _ = graph2.insert_tx(tx2.clone());
    let _ = graph2.insert_tx(tx1);

    assert_eq!(
        graph1.outspends(op),
        &iter::once(tx2.compute_txid()).collect::<HashSet<_>>()
    );
    assert_eq!(graph2.outspends(op), graph1.outspends(op));
}

#[test]
fn insert_tx_can_retrieve_full_tx_from_graph() {
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut::NULL],
    };

    let mut graph = TxGraph::<ConfirmationBlockTime>::default();
    let _ = graph.insert_tx(tx.clone());
    assert_eq!(
        graph
            .get_tx(tx.compute_txid())
            .map(|tx| tx.as_ref().clone()),
        Some(tx)
    );
}

#[test]
fn insert_tx_displaces_txouts() {
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();

    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(42_000),
            script_pubkey: ScriptBuf::default(),
        }],
    };
    let txid = tx.compute_txid();
    let outpoint = OutPoint::new(txid, 0);
    let txout = tx.output.first().unwrap();

    let changeset = tx_graph.insert_txout(outpoint, txout.clone());
    assert!(!changeset.is_empty());

    let changeset = tx_graph.insert_tx(tx.clone());
    assert_eq!(changeset.txs.len(), 1);
    assert!(changeset.txouts.is_empty());
    assert!(tx_graph.get_tx(txid).is_some());
    assert_eq!(tx_graph.get_txout(outpoint), Some(txout));
}

#[test]
fn insert_txout_does_not_displace_tx() {
    let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(42_000),
            script_pubkey: ScriptBuf::new(),
        }],
    };

    let _changeset = tx_graph.insert_tx(tx.clone());

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.compute_txid(),
            vout: 0,
        },
        TxOut {
            value: Amount::from_sat(1_337_000),
            script_pubkey: ScriptBuf::new(),
        },
    );

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.compute_txid(),
            vout: 1,
        },
        TxOut {
            value: Amount::from_sat(1_000_000_000),
            script_pubkey: ScriptBuf::new(),
        },
    );

    assert_eq!(
        tx_graph
            .get_txout(OutPoint {
                txid: tx.compute_txid(),
                vout: 0
            })
            .unwrap()
            .value,
        Amount::from_sat(42_000)
    );
    assert_eq!(
        tx_graph.get_txout(OutPoint {
            txid: tx.compute_txid(),
            vout: 1
        }),
        None
    );
}

#[test]
fn test_calculate_fee() {
    let mut graph = TxGraph::<ConfirmationBlockTime>::default();
    let intx1 = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(100),
            script_pubkey: ScriptBuf::new(),
        }],
    };
    let intx2 = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(200),
            script_pubkey: ScriptBuf::new(),
        }],
    };

    let intxout1 = (
        OutPoint {
            txid: hash!("dangling output"),
            vout: 0,
        },
        TxOut {
            value: Amount::from_sat(300),
            script_pubkey: ScriptBuf::new(),
        },
    );

    let _ = graph.insert_tx(intx1.clone());
    let _ = graph.insert_tx(intx2.clone());
    let _ = graph.insert_txout(intxout1.0, intxout1.1);

    let mut tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint {
                    txid: intx1.compute_txid(),
                    vout: 0,
                },
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint {
                    txid: intx2.compute_txid(),
                    vout: 0,
                },
                ..Default::default()
            },
            TxIn {
                previous_output: intxout1.0,
                ..Default::default()
            },
        ],
        output: vec![TxOut {
            value: Amount::from_sat(500),
            script_pubkey: ScriptBuf::new(),
        }],
    };

    assert_eq!(graph.calculate_fee(&tx), Ok(Amount::from_sat(100)));

    tx.input.remove(2);

    // fee would be negative, should return CalculateFeeError::NegativeFee
    assert_eq!(
        graph.calculate_fee(&tx),
        Err(CalculateFeeError::NegativeFee(SignedAmount::from_sat(-200)))
    );

    // If we have an unknown outpoint, fee should return CalculateFeeError::MissingTxOut.
    let outpoint = OutPoint {
        txid: hash!("unknown_txid"),
        vout: 0,
    };
    tx.input.push(TxIn {
        previous_output: outpoint,
        ..Default::default()
    });
    assert_eq!(
        graph.calculate_fee(&tx),
        Err(CalculateFeeError::MissingTxOut(vec!(outpoint)))
    );
}

#[test]
fn test_calculate_fee_on_coinbase() {
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut::NULL],
    };

    let graph = TxGraph::<()>::default();

    assert_eq!(graph.calculate_fee(&tx), Ok(Amount::ZERO));
}

// `test_walk_ancestors` uses the following transaction structure:
//
//     a0
//    /  \
//   b0   b1   b2
//  /  \   \  /
// c0  c1   c2  c3
//    /      \  /
//   d0       d1
//             \
//              e0
//
// where b0 and b1 spend a0, c0 and c1 spend b0, d0 spends c1, etc.
#[test]
fn test_walk_ancestors() {
    let local_chain = LocalChain::from_blocks(
        (0..=20)
            .map(|ht| (ht, BlockHash::hash(format!("Block Hash {}", ht).as_bytes())))
            .collect(),
    )
    .expect("must contain genesis hash");
    let tip = local_chain.tip();

    let tx_a0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("op0"), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL, TxOut::NULL],
        ..new_tx(0)
    };

    // tx_b0 spends tx_a0
    let tx_b0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a0.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL, TxOut::NULL],
        ..new_tx(0)
    };

    // tx_b1 spends tx_a0
    let tx_b1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a0.compute_txid(), 1),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    let tx_b2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("op1"), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_c0 spends tx_b0
    let tx_c0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_b0.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_c1 spends tx_b0
    let tx_c1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_b0.compute_txid(), 1),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_c2 spends tx_b1 and tx_b2
    let tx_c2 = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(tx_b1.compute_txid(), 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(tx_b2.compute_txid(), 0),
                ..TxIn::default()
            },
        ],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    let tx_c3 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("op2"), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_d0 spends tx_c1
    let tx_d0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_c1.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_d1 spends tx_c2 and tx_c3
    let tx_d1 = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(tx_c2.compute_txid(), 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(tx_c3.compute_txid(), 0),
                ..TxIn::default()
            },
        ],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_e0 spends tx_d1
    let tx_e0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_d1.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    let mut graph = TxGraph::<BlockId>::new([
        tx_a0.clone(),
        tx_b0.clone(),
        tx_b1.clone(),
        tx_b2.clone(),
        tx_c0.clone(),
        tx_c1.clone(),
        tx_c2.clone(),
        tx_c3.clone(),
        tx_d0.clone(),
        tx_d1.clone(),
        tx_e0.clone(),
    ]);

    [&tx_a0, &tx_b1].iter().for_each(|&tx| {
        let changeset = graph.insert_anchor(tx.compute_txid(), tip.block_id());
        assert!(!changeset.is_empty());
    });

    let ancestors = [
        graph
            .walk_ancestors(tx_c0.clone(), |depth, tx| Some((depth, tx)))
            .collect::<Vec<_>>(),
        graph
            .walk_ancestors(tx_d0.clone(), |depth, tx| Some((depth, tx)))
            .collect::<Vec<_>>(),
        graph
            .walk_ancestors(tx_e0.clone(), |depth, tx| Some((depth, tx)))
            .collect::<Vec<_>>(),
        // Only traverse unconfirmed ancestors of tx_e0 this time
        graph
            .walk_ancestors(tx_e0.clone(), |depth, tx| {
                let tx_node = graph.get_tx_node(tx.compute_txid())?;
                for block in tx_node.anchors {
                    match local_chain.is_block_in_chain(block.anchor_block(), tip.block_id()) {
                        Ok(Some(true)) => return None,
                        _ => continue,
                    }
                }
                Some((depth, tx_node.tx))
            })
            .collect::<Vec<_>>(),
    ];

    let expected_ancestors = [
        vec![(1, &tx_b0), (2, &tx_a0)],
        vec![(1, &tx_c1), (2, &tx_b0), (3, &tx_a0)],
        vec![
            (1, &tx_d1),
            (2, &tx_c2),
            (2, &tx_c3),
            (3, &tx_b1),
            (3, &tx_b2),
            (4, &tx_a0),
        ],
        vec![(1, &tx_d1), (2, &tx_c2), (2, &tx_c3), (3, &tx_b2)],
    ];

    for (txids, expected_txids) in ancestors.into_iter().zip(expected_ancestors) {
        assert_eq!(
            txids,
            expected_txids
                .into_iter()
                .map(|(i, tx)| (i, Arc::new(tx.clone())))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_conflicting_descendants() {
    let previous_output = OutPoint::new(hash!("op"), 2);

    // tx_a spends previous_output
    let tx_a = Transaction {
        input: vec![TxIn {
            previous_output,
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(0)
    };

    // tx_a2 spends previous_output and conflicts with tx_a
    let tx_a2 = Transaction {
        input: vec![TxIn {
            previous_output,
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL, TxOut::NULL],
        ..new_tx(1)
    };

    // tx_b spends tx_a
    let tx_b = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(2)
    };

    let txid_a = tx_a.compute_txid();
    let txid_b = tx_b.compute_txid();

    let mut graph = TxGraph::<ConfirmationBlockTime>::default();
    let _ = graph.insert_tx(tx_a);
    let _ = graph.insert_tx(tx_b);

    assert_eq!(
        graph
            .walk_conflicts(&tx_a2, |depth, txid| Some((depth, txid)))
            .collect::<Vec<_>>(),
        vec![(0_usize, txid_a), (1_usize, txid_b),],
    );
}

#[test]
fn test_descendants_no_repeat() {
    let tx_a = Transaction {
        output: vec![TxOut::NULL, TxOut::NULL, TxOut::NULL],
        ..new_tx(0)
    };

    let txs_b = (0..3)
        .map(|vout| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(tx_a.compute_txid(), vout),
                ..TxIn::default()
            }],
            output: vec![TxOut::NULL],
            ..new_tx(1)
        })
        .collect::<Vec<_>>();

    let txs_c = (0..2)
        .map(|vout| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(txs_b[vout as usize].compute_txid(), vout),
                ..TxIn::default()
            }],
            output: vec![TxOut::NULL],
            ..new_tx(2)
        })
        .collect::<Vec<_>>();

    let tx_d = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(txs_c[0].compute_txid(), 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(txs_c[1].compute_txid(), 0),
                ..TxIn::default()
            },
        ],
        output: vec![TxOut::NULL],
        ..new_tx(3)
    };

    let tx_e = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_d.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::NULL],
        ..new_tx(4)
    };

    let txs_not_connected = (10..20)
        .map(|v| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(hash!("tx_does_not_exist"), v),
                ..TxIn::default()
            }],
            output: vec![TxOut::NULL],
            ..new_tx(v)
        })
        .collect::<Vec<_>>();

    let mut graph = TxGraph::<ConfirmationBlockTime>::default();
    let mut expected_txids = Vec::new();

    // these are NOT descendants of `tx_a`
    for tx in txs_not_connected {
        let _ = graph.insert_tx(tx.clone());
    }

    // these are the expected descendants of `tx_a`
    for tx in txs_b
        .iter()
        .chain(&txs_c)
        .chain(core::iter::once(&tx_d))
        .chain(core::iter::once(&tx_e))
    {
        let _ = graph.insert_tx(tx.clone());
        expected_txids.push(tx.compute_txid());
    }

    let descendants = graph
        .walk_descendants(tx_a.compute_txid(), |_, txid| Some(txid))
        .collect::<Vec<_>>();

    assert_eq!(descendants, expected_txids);
}

#[test]
fn test_chain_spends() {
    let local_chain = LocalChain::from_blocks(
        (0..=100)
            .map(|ht| (ht, BlockHash::hash(format!("Block Hash {}", ht).as_bytes())))
            .collect(),
    )
    .expect("must have genesis hash");
    let tip = local_chain.tip();

    // The parent tx contains 2 outputs. Which are spent by one confirmed and one unconfirmed tx.
    // The parent tx is confirmed at block 95.
    let tx_0 = Transaction {
        input: vec![],
        output: vec![
            TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..new_tx(0)
    };

    // The first confirmed transaction spends vout: 0. And is confirmed at block 98.
    let tx_1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.compute_txid(), 0),
            ..TxIn::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(5_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(5_000),
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..new_tx(0)
    };

    // The second transactions spends vout:1, and is unconfirmed.
    let tx_2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.compute_txid(), 1),
            ..TxIn::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..new_tx(0)
    };

    let mut graph = TxGraph::<ConfirmationBlockTime>::default();

    let _ = graph.insert_tx(tx_0.clone());
    let _ = graph.insert_tx(tx_1.clone());
    let _ = graph.insert_tx(tx_2.clone());

    for (ht, tx) in [(95, &tx_0), (98, &tx_1)] {
        let _ = graph.insert_anchor(
            tx.compute_txid(),
            ConfirmationBlockTime {
                block_id: tip.get(ht).unwrap().block_id(),
                confirmation_time: 100,
            },
        );
    }

    let build_canonical_spends =
        |chain: &LocalChain, tx_graph: &TxGraph<ConfirmationBlockTime>| -> HashMap<OutPoint, _> {
            tx_graph
                .filter_chain_txouts(
                    chain,
                    tip.block_id(),
                    tx_graph.all_txouts().map(|(op, _)| ((), op)),
                )
                .filter_map(|(_, full_txo)| Some((full_txo.outpoint, full_txo.spent_by?)))
                .collect()
        };
    let build_canonical_positions = |chain: &LocalChain,
                                     tx_graph: &TxGraph<ConfirmationBlockTime>|
     -> HashMap<Txid, ChainPosition<ConfirmationBlockTime>> {
        tx_graph
            .list_canonical_txs(chain, tip.block_id())
            .map(|canon_tx| (canon_tx.tx_node.txid, canon_tx.chain_position))
            .collect()
    };

    {
        let canonical_spends = build_canonical_spends(&local_chain, &graph);
        let canonical_positions = build_canonical_positions(&local_chain, &graph);

        // Assert that confirmed spends are returned correctly.
        assert_eq!(
            canonical_spends
                .get(&OutPoint::new(tx_0.compute_txid(), 0))
                .cloned(),
            Some((
                ChainPosition::Confirmed {
                    anchor: ConfirmationBlockTime {
                        block_id: tip.get(98).unwrap().block_id(),
                        confirmation_time: 100
                    },
                    transitively: None,
                },
                tx_1.compute_txid(),
            )),
        );
        // Check if chain position is returned correctly.
        assert_eq!(
            canonical_positions.get(&tx_0.compute_txid()).cloned(),
            Some(ChainPosition::Confirmed {
                anchor: ConfirmationBlockTime {
                    block_id: tip.get(95).unwrap().block_id(),
                    confirmation_time: 100
                },
                transitively: None
            })
        );
    }

    // Mark the unconfirmed as seen and check correct ObservedAs status is returned.
    let _ = graph.insert_seen_at(tx_2.compute_txid(), 1234567);
    {
        let canonical_spends = build_canonical_spends(&local_chain, &graph);

        // Check chain spend returned correctly.
        assert_eq!(
            canonical_spends
                .get(&OutPoint::new(tx_0.compute_txid(), 1))
                .cloned(),
            Some((
                ChainPosition::Unconfirmed {
                    last_seen: Some(1234567)
                },
                tx_2.compute_txid()
            ))
        );
    }

    // A conflicting transaction that conflicts with tx_1.
    let tx_1_conflict = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.compute_txid(), 0),
            ..Default::default()
        }],
        ..new_tx(0)
    };
    let _ = graph.insert_tx(tx_1_conflict.clone());
    {
        let canonical_positions = build_canonical_positions(&local_chain, &graph);

        // Because this tx conflicts with an already confirmed transaction, chain position should return none.
        assert!(canonical_positions
            .get(&tx_1_conflict.compute_txid())
            .is_none());
    }

    // Another conflicting tx that conflicts with tx_2.
    let tx_2_conflict = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.compute_txid(), 1),
            ..Default::default()
        }],
        ..new_tx(0)
    };
    // Insert in graph and mark it as seen.
    let _ = graph.insert_tx(tx_2_conflict.clone());
    let _ = graph.insert_seen_at(tx_2_conflict.compute_txid(), 1234568);
    {
        let canonical_spends = build_canonical_spends(&local_chain, &graph);
        let canonical_positions = build_canonical_positions(&local_chain, &graph);

        // This should return a valid observation with correct last seen.
        assert_eq!(
            canonical_positions
                .get(&tx_2_conflict.compute_txid())
                .cloned(),
            Some(ChainPosition::Unconfirmed {
                last_seen: Some(1234568)
            })
        );

        // Chain_spend now catches the new transaction as the spend.
        assert_eq!(
            canonical_spends
                .get(&OutPoint::new(tx_0.compute_txid(), 1))
                .cloned(),
            Some((
                ChainPosition::Unconfirmed {
                    last_seen: Some(1234568)
                },
                tx_2_conflict.compute_txid()
            ))
        );

        // Chain position of the `tx_2` is now none, as it is older than `tx_2_conflict`
        assert!(canonical_positions.get(&tx_2.compute_txid()).is_none());
    }
}

/// Ensure that `last_seen` values only increase during [`Merge::merge`].
#[test]
fn test_changeset_last_seen_merge() {
    let txid: Txid = hash!("test txid");

    let test_cases: &[(Option<u64>, Option<u64>)] = &[
        (Some(5), Some(6)),
        (Some(5), Some(5)),
        (Some(6), Some(5)),
        (None, Some(5)),
        (Some(5), None),
    ];

    for (original_ls, update_ls) in test_cases {
        let mut original = ChangeSet::<()> {
            last_seen: original_ls.map(|ls| (txid, ls)).into_iter().collect(),
            ..Default::default()
        };
        assert!(!original.is_empty() || original_ls.is_none());
        let update = ChangeSet::<()> {
            last_seen: update_ls.map(|ls| (txid, ls)).into_iter().collect(),
            ..Default::default()
        };
        assert!(!update.is_empty() || update_ls.is_none());

        original.merge(update);
        assert_eq!(
            &original.last_seen.get(&txid).cloned(),
            Ord::max(original_ls, update_ls),
        );
    }
}

#[test]
fn transactions_inserted_into_tx_graph_are_not_canonical_until_they_have_an_anchor_in_best_chain() {
    let txs = vec![new_tx(0), new_tx(1)];
    let txids: Vec<Txid> = txs.iter().map(Transaction::compute_txid).collect();

    // graph
    let mut graph = TxGraph::<BlockId>::new(txs);
    let full_txs: Vec<_> = graph.full_txs().collect();
    assert_eq!(full_txs.len(), 2);
    let unseen_txs: Vec<_> = graph.txs_with_no_anchor_or_last_seen().collect();
    assert_eq!(unseen_txs.len(), 2);

    // chain
    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("g")), (1, hash!("A")), (2, hash!("B"))]
        .into_iter()
        .collect();
    let chain = LocalChain::from_blocks(blocks).unwrap();
    let canonical_txs: Vec<_> = graph
        .list_canonical_txs(&chain, chain.tip().block_id())
        .collect();
    assert!(canonical_txs.is_empty());

    // tx0 with seen_at should be returned by canonical txs
    let _ = graph.insert_seen_at(txids[0], 2);
    let mut canonical_txs = graph.list_canonical_txs(&chain, chain.tip().block_id());
    assert_eq!(
        canonical_txs.next().map(|tx| tx.tx_node.txid).unwrap(),
        txids[0]
    );
    drop(canonical_txs);

    // tx1 with anchor is also canonical
    let _ = graph.insert_anchor(txids[1], block_id!(2, "B"));
    let canonical_txids: Vec<_> = graph
        .list_canonical_txs(&chain, chain.tip().block_id())
        .map(|tx| tx.tx_node.txid)
        .collect();
    assert!(canonical_txids.contains(&txids[1]));
    assert!(graph.txs_with_no_anchor_or_last_seen().next().is_none());
}

#[test]
fn insert_anchor_without_tx() {
    let mut graph = TxGraph::<BlockId>::default();

    let tx = new_tx(21);
    let txid = tx.compute_txid();

    let anchor = BlockId {
        height: 100,
        hash: hash!("A"),
    };

    // insert anchor with no corresponding tx
    let mut changeset = graph.insert_anchor(txid, anchor);
    assert!(changeset.anchors.contains(&(anchor, txid)));
    // recover from changeset
    let mut recovered = TxGraph::default();
    recovered.apply_changeset(changeset.clone());
    assert_eq!(recovered, graph);

    // now insert tx
    let tx = Arc::new(tx);
    let graph_changeset = graph.insert_tx(tx.clone());
    assert!(graph_changeset.txs.contains(&tx));
    changeset.merge(graph_changeset);
    // recover from changeset again
    let mut recovered = TxGraph::default();
    recovered.apply_changeset(changeset);
    assert_eq!(recovered, graph);
}

#[test]
/// The `map_anchors` allow a caller to pass a function to reconstruct the [`TxGraph`] with any [`Anchor`],
/// even though the function is non-deterministic.
fn call_map_anchors_with_non_deterministic_anchor() {
    #[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
    /// A non-deterministic anchor
    pub struct NonDeterministicAnchor {
        pub anchor_block: BlockId,
        pub non_deterministic_field: u32,
    }

    impl Anchor for NonDeterministicAnchor {
        fn anchor_block(&self) -> BlockId {
            self.anchor_block
        }
    }

    let template = [
        TxTemplate {
            tx_name: "tx1",
            inputs: &[TxInTemplate::Bogus],
            outputs: &[TxOutTemplate::new(10000, Some(1))],
            anchors: &[block_id!(1, "A")],
            last_seen: None,
        },
        TxTemplate {
            tx_name: "tx2",
            inputs: &[TxInTemplate::PrevTx("tx1", 0)],
            outputs: &[TxOutTemplate::new(20000, Some(2))],
            anchors: &[block_id!(2, "B")],
            ..Default::default()
        },
        TxTemplate {
            tx_name: "tx3",
            inputs: &[TxInTemplate::PrevTx("tx2", 0)],
            outputs: &[TxOutTemplate::new(30000, Some(3))],
            anchors: &[block_id!(3, "C"), block_id!(4, "D")],
            ..Default::default()
        },
    ];
    let (graph, _, _) = init_graph(&template);
    let new_graph = graph.clone().map_anchors(|a| NonDeterministicAnchor {
        anchor_block: a,
        // A non-deterministic value
        non_deterministic_field: rand::thread_rng().next_u32(),
    });

    // Check all the details in new_graph reconstruct as well

    let mut full_txs_vec: Vec<_> = graph.full_txs().collect();
    full_txs_vec.sort();
    let mut new_txs_vec: Vec<_> = new_graph.full_txs().collect();
    new_txs_vec.sort();
    let mut new_txs = new_txs_vec.iter();

    for tx_node in full_txs_vec.iter() {
        let new_txnode = new_txs.next().unwrap();
        assert_eq!(new_txnode.txid, tx_node.txid);
        assert_eq!(new_txnode.tx, tx_node.tx);
        assert_eq!(
            new_txnode.last_seen_unconfirmed,
            tx_node.last_seen_unconfirmed
        );
        assert_eq!(new_txnode.anchors.len(), tx_node.anchors.len());

        let mut new_anchors: Vec<_> = new_txnode.anchors.iter().map(|a| a.anchor_block).collect();
        new_anchors.sort();
        let mut old_anchors: Vec<_> = tx_node.anchors.iter().copied().collect();
        old_anchors.sort();
        assert_eq!(new_anchors, old_anchors);
    }
    assert!(new_txs.next().is_none());

    let mut new_graph_anchors: Vec<_> = new_graph
        .all_anchors()
        .iter()
        .flat_map(|(_, anchors)| anchors)
        .map(|a| a.anchor_block)
        .collect();
    new_graph_anchors.sort();
    assert_eq!(
        new_graph_anchors,
        vec![
            block_id!(1, "A"),
            block_id!(2, "B"),
            block_id!(3, "C"),
            block_id!(4, "D"),
        ]
    );
}

/// Tests `From` impls for conversion between [`TxGraph`] and [`tx_graph::TxUpdate`].
#[test]
fn tx_graph_update_conversion() {
    use tx_graph::TxUpdate;

    type TestCase = (&'static str, TxUpdate<ConfirmationBlockTime>);

    fn make_tx(v: i32) -> Transaction {
        Transaction {
            version: transaction::Version(v),
            lock_time: absolute::LockTime::ZERO,
            input: vec![],
            output: vec![],
        }
    }

    fn make_txout(a: u64) -> TxOut {
        TxOut {
            value: Amount::from_sat(a),
            script_pubkey: ScriptBuf::default(),
        }
    }

    let test_cases: &[TestCase] = &[
        ("empty_update", TxUpdate::default()),
        ("single_tx", {
            let mut tx_update = TxUpdate::default();
            tx_update.txs = vec![make_tx(0).into()];
            tx_update
        }),
        ("two_txs", {
            let mut tx_update = TxUpdate::default();
            tx_update.txs = vec![make_tx(0).into(), make_tx(1).into()];
            tx_update
        }),
        ("with_floating_txouts", {
            let mut tx_update = TxUpdate::default();
            tx_update.txs = vec![make_tx(0).into(), make_tx(1).into()];
            tx_update.txouts = [
                (OutPoint::new(hash!("a"), 0), make_txout(0)),
                (OutPoint::new(hash!("a"), 1), make_txout(1)),
                (OutPoint::new(hash!("b"), 0), make_txout(2)),
            ]
            .into();
            tx_update
        }),
        ("with_anchors", {
            let mut tx_update = TxUpdate::default();
            tx_update.txs = vec![make_tx(0).into(), make_tx(1).into()];
            tx_update.txouts = [
                (OutPoint::new(hash!("a"), 0), make_txout(0)),
                (OutPoint::new(hash!("a"), 1), make_txout(1)),
                (OutPoint::new(hash!("b"), 0), make_txout(2)),
            ]
            .into();
            tx_update.anchors = [
                (ConfirmationBlockTime::default(), hash!("a")),
                (ConfirmationBlockTime::default(), hash!("b")),
            ]
            .into();
            tx_update
        }),
        ("with_seen_ats", {
            let mut tx_update = TxUpdate::default();
            tx_update.txs = vec![make_tx(0).into(), make_tx(1).into()];
            tx_update.txouts = [
                (OutPoint::new(hash!("a"), 0), make_txout(0)),
                (OutPoint::new(hash!("a"), 1), make_txout(1)),
                (OutPoint::new(hash!("d"), 0), make_txout(2)),
            ]
            .into();
            tx_update.anchors = [
                (ConfirmationBlockTime::default(), hash!("a")),
                (ConfirmationBlockTime::default(), hash!("b")),
            ]
            .into();
            tx_update.seen_ats = [(hash!("c"), 12346)].into_iter().collect();
            tx_update
        }),
    ];

    for (test_name, update) in test_cases {
        let mut tx_graph = TxGraph::<ConfirmationBlockTime>::default();
        let _ = tx_graph.apply_update_at(update.clone(), None);
        let update_from_tx_graph: TxUpdate<ConfirmationBlockTime> = tx_graph.into();

        assert_eq!(
            update
                .txs
                .iter()
                .map(|tx| tx.compute_txid())
                .collect::<HashSet<Txid>>(),
            update_from_tx_graph
                .txs
                .iter()
                .map(|tx| tx.compute_txid())
                .collect::<HashSet<Txid>>(),
            "{}: txs do not match",
            test_name
        );
        assert_eq!(
            update.txouts, update_from_tx_graph.txouts,
            "{}: txouts do not match",
            test_name
        );
        assert_eq!(
            update.anchors, update_from_tx_graph.anchors,
            "{}: anchors do not match",
            test_name
        );
        assert_eq!(
            update.seen_ats, update_from_tx_graph.seen_ats,
            "{}: seen_ats do not match",
            test_name
        );
    }
}
