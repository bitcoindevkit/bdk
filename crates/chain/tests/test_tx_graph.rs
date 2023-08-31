#[macro_use]
mod common;
use bdk_chain::tx_graph::CalculateFeeError;
use bdk_chain::{
    collections::*,
    local_chain::LocalChain,
    tx_graph::{ChangeSet, TxGraph},
    Anchor, Append, BlockId, ChainPosition, ConfirmationHeightAnchor,
};
use bitcoin::{
    absolute, hashes::Hash, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid,
};
use core::iter;
use std::vec;

#[test]
fn insert_txouts() {
    // 2 (Outpoint, TxOut) tupples that denotes original data in the graph, as partial transactions.
    let original_ops = [
        (
            OutPoint::new(h!("tx1"), 1),
            TxOut {
                value: 10_000,
                script_pubkey: ScriptBuf::new(),
            },
        ),
        (
            OutPoint::new(h!("tx1"), 2),
            TxOut {
                value: 20_000,
                script_pubkey: ScriptBuf::new(),
            },
        ),
    ];

    // Another (OutPoint, TxOut) tupple to be used as update as partial transaction.
    let update_ops = [(
        OutPoint::new(h!("tx2"), 0),
        TxOut {
            value: 20_000,
            script_pubkey: ScriptBuf::new(),
        },
    )];

    // One full transaction to be included in the update
    let update_txs = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 30_000,
            script_pubkey: ScriptBuf::new(),
        }],
    };

    // Conf anchor used to mark the full transaction as confirmed.
    let conf_anchor = ChainPosition::Confirmed(BlockId {
        height: 100,
        hash: h!("random blockhash"),
    });

    // Unconfirmed anchor to mark the partial transactions as unconfirmed
    let unconf_anchor = ChainPosition::<BlockId>::Unconfirmed(1000000);

    // Make the original graph
    let mut graph = {
        let mut graph = TxGraph::<ChainPosition<BlockId>>::default();
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
        let mut graph = TxGraph::default();
        for (outpoint, txout) in &update_ops {
            // Insert partials transactions
            assert_eq!(
                graph.insert_txout(*outpoint, txout.clone()),
                ChangeSet {
                    txouts: [(*outpoint, txout.clone())].into(),
                    ..Default::default()
                }
            );
            // Mark them unconfirmed.
            assert_eq!(
                graph.insert_anchor(outpoint.txid, unconf_anchor),
                ChangeSet {
                    txs: [].into(),
                    txouts: [].into(),
                    anchors: [(unconf_anchor, outpoint.txid)].into(),
                    last_seen: [].into()
                }
            );
            // Mark them last seen at.
            assert_eq!(
                graph.insert_seen_at(outpoint.txid, 1000000),
                ChangeSet {
                    txs: [].into(),
                    txouts: [].into(),
                    anchors: [].into(),
                    last_seen: [(outpoint.txid, 1000000)].into()
                }
            );
        }
        // Insert the full transaction
        assert_eq!(
            graph.insert_tx(update_txs.clone()),
            ChangeSet {
                txs: [update_txs.clone()].into(),
                ..Default::default()
            }
        );

        // Mark it as confirmed.
        assert_eq!(
            graph.insert_anchor(update_txs.txid(), conf_anchor),
            ChangeSet {
                txs: [].into(),
                txouts: [].into(),
                anchors: [(conf_anchor, update_txs.txid())].into(),
                last_seen: [].into()
            }
        );
        graph
    };

    // Check the resulting addition.
    let changeset = graph.apply_update(update);

    assert_eq!(
        changeset,
        ChangeSet {
            txs: [update_txs.clone()].into(),
            txouts: update_ops.clone().into(),
            anchors: [(conf_anchor, update_txs.txid()), (unconf_anchor, h!("tx2"))].into(),
            last_seen: [(h!("tx2"), 1000000)].into()
        }
    );

    // Apply changeset and check the new graph counts.
    graph.apply_changeset(changeset);
    assert_eq!(graph.all_txouts().count(), 4);
    assert_eq!(graph.full_txs().count(), 1);
    assert_eq!(graph.floating_txouts().count(), 3);

    // Check TxOuts are fetched correctly from the graph.
    assert_eq!(
        graph.tx_outputs(h!("tx1")).expect("should exists"),
        [
            (
                1u32,
                &TxOut {
                    value: 10_000,
                    script_pubkey: ScriptBuf::new(),
                }
            ),
            (
                2u32,
                &TxOut {
                    value: 20_000,
                    script_pubkey: ScriptBuf::new(),
                }
            )
        ]
        .into()
    );

    assert_eq!(
        graph.tx_outputs(update_txs.txid()).expect("should exists"),
        [(
            0u32,
            &TxOut {
                value: 30_000,
                script_pubkey: ScriptBuf::new()
            }
        )]
        .into()
    );

    // Check that the initial_changeset is correct
    assert_eq!(
        graph.initial_changeset(),
        ChangeSet {
            txs: [update_txs.clone()].into(),
            txouts: update_ops.into_iter().chain(original_ops).collect(),
            anchors: [(conf_anchor, update_txs.txid()), (unconf_anchor, h!("tx2"))].into(),
            last_seen: [(h!("tx2"), 1000000)].into()
        }
    );
}

#[test]
fn insert_tx_graph_doesnt_count_coinbase_as_spent() {
    let tx = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![],
    };

    let mut graph = TxGraph::<()>::default();
    let _ = graph.insert_tx(tx);
    assert!(graph.outspends(OutPoint::null()).is_empty());
    assert!(graph.tx_spends(Txid::all_zeros()).next().is_none());
}

#[test]
fn insert_tx_graph_keeps_track_of_spend() {
    let tx1 = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut::default()],
    };

    let op = OutPoint {
        txid: tx1.txid(),
        vout: 0,
    };

    let tx2 = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: op,
            ..Default::default()
        }],
        output: vec![],
    };

    let mut graph1 = TxGraph::<()>::default();
    let mut graph2 = TxGraph::<()>::default();

    // insert in different order
    let _ = graph1.insert_tx(tx1.clone());
    let _ = graph1.insert_tx(tx2.clone());

    let _ = graph2.insert_tx(tx2.clone());
    let _ = graph2.insert_tx(tx1);

    assert_eq!(
        graph1.outspends(op),
        &iter::once(tx2.txid()).collect::<HashSet<_>>()
    );
    assert_eq!(graph2.outspends(op), graph1.outspends(op));
}

#[test]
fn insert_tx_can_retrieve_full_tx_from_graph() {
    let tx = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut::default()],
    };

    let mut graph = TxGraph::<()>::default();
    let _ = graph.insert_tx(tx.clone());
    assert_eq!(graph.get_tx(tx.txid()), Some(&tx));
}

#[test]
fn insert_tx_displaces_txouts() {
    let mut tx_graph = TxGraph::<()>::default();
    let tx = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: 42_000,
            script_pubkey: ScriptBuf::default(),
        }],
    };

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1_337_000,
            script_pubkey: ScriptBuf::default(),
        },
    );

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1_000_000_000,
            script_pubkey: ScriptBuf::default(),
        },
    );

    let _changeset = tx_graph.insert_tx(tx.clone());

    assert_eq!(
        tx_graph
            .get_txout(OutPoint {
                txid: tx.txid(),
                vout: 0
            })
            .unwrap()
            .value,
        42_000
    );
    assert_eq!(
        tx_graph.get_txout(OutPoint {
            txid: tx.txid(),
            vout: 1
        }),
        None
    );
}

#[test]
fn insert_txout_does_not_displace_tx() {
    let mut tx_graph = TxGraph::<()>::default();
    let tx = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: 42_000,
            script_pubkey: ScriptBuf::default(),
        }],
    };

    let _changeset = tx_graph.insert_tx(tx.clone());

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1_337_000,
            script_pubkey: ScriptBuf::default(),
        },
    );

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1_000_000_000,
            script_pubkey: ScriptBuf::default(),
        },
    );

    assert_eq!(
        tx_graph
            .get_txout(OutPoint {
                txid: tx.txid(),
                vout: 0
            })
            .unwrap()
            .value,
        42_000
    );
    assert_eq!(
        tx_graph.get_txout(OutPoint {
            txid: tx.txid(),
            vout: 1
        }),
        None
    );
}

#[test]
fn test_calculate_fee() {
    let mut graph = TxGraph::<()>::default();
    let intx1 = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: 100,
            ..Default::default()
        }],
    };
    let intx2 = Transaction {
        version: 0x02,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: 200,
            ..Default::default()
        }],
    };

    let intxout1 = (
        OutPoint {
            txid: h!("dangling output"),
            vout: 0,
        },
        TxOut {
            value: 300,
            ..Default::default()
        },
    );

    let _ = graph.insert_tx(intx1.clone());
    let _ = graph.insert_tx(intx2.clone());
    let _ = graph.insert_txout(intxout1.0, intxout1.1);

    let mut tx = Transaction {
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint {
                    txid: intx1.txid(),
                    vout: 0,
                },
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint {
                    txid: intx2.txid(),
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
            value: 500,
            ..Default::default()
        }],
    };

    assert_eq!(graph.calculate_fee(&tx), Ok(100));

    tx.input.remove(2);

    // fee would be negative, should return CalculateFeeError::NegativeFee
    assert_eq!(
        graph.calculate_fee(&tx),
        Err(CalculateFeeError::NegativeFee(-200))
    );

    // If we have an unknown outpoint, fee should return CalculateFeeError::MissingTxOut.
    let outpoint = OutPoint {
        txid: h!("unknown_txid"),
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
        version: 0x01,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut::default()],
    };

    let graph = TxGraph::<()>::default();

    assert_eq!(graph.calculate_fee(&tx), Ok(0));
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
    let tx_a0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(h!("op0"), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default(), TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_b0 spends tx_a0
    let tx_b0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a0.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default(), TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_b1 spends tx_a0
    let tx_b1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a0.txid(), 1),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    let tx_b2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(h!("op1"), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_c0 spends tx_b0
    let tx_c0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_b0.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_c1 spends tx_b0
    let tx_c1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_b0.txid(), 1),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_c2 spends tx_b1 and tx_b2
    let tx_c2 = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(tx_b1.txid(), 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(tx_b2.txid(), 0),
                ..TxIn::default()
            },
        ],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    let tx_c3 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(h!("op2"), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_d0 spends tx_c1
    let tx_d0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_c1.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_d1 spends tx_c2 and tx_c3
    let tx_d1 = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(tx_c2.txid(), 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(tx_c3.txid(), 0),
                ..TxIn::default()
            },
        ],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_e0 spends tx_d1
    let tx_e0 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_d1.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    let graph = TxGraph::<()>::new(vec![
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

    let mut ancestors = vec![
        graph
            .walk_ancestors(tx_e0.clone(), 25)
            .collect::<Vec<_>>()
            .iter()
            .map(|(depth, tx)| (*depth, tx.txid()))
            .collect::<Vec<_>>(),
        graph
            .walk_ancestors(tx_e0, 3)
            .collect::<Vec<_>>()
            .iter()
            .map(|(depth, tx)| (*depth, tx.txid()))
            .collect::<Vec<_>>(),
        graph
            .walk_ancestors(tx_d0, 5)
            .collect::<Vec<_>>()
            .iter()
            .map(|(depth, tx)| (*depth, tx.txid()))
            .collect::<Vec<_>>(),
        graph
            .walk_ancestors(tx_c0, 1)
            .collect::<Vec<_>>()
            .iter()
            .map(|(depth, tx)| (*depth, tx.txid()))
            .collect::<Vec<_>>(),
    ];

    let mut expected_ancestors: Vec<Vec<(usize, Txid)>> = vec![
        vec![
            (4, tx_a0.txid()),
            (3, tx_b1.txid()),
            (3, tx_b2.txid()),
            (2, tx_c2.txid()),
            (2, tx_c3.txid()),
            (1, tx_d1.txid()),
        ],
        vec![
            (3, tx_b1.txid()),
            (3, tx_b2.txid()),
            (2, tx_c2.txid()),
            (2, tx_c3.txid()),
            (1, tx_d1.txid()),
        ],
        vec![(3, tx_a0.txid()), (2, tx_b0.txid()), (1, tx_c1.txid())],
        vec![(1, tx_b0.txid())],
    ];

    while let Some(mut txids) = ancestors.pop() {
        let mut expected_txids = expected_ancestors.pop().unwrap();
        txids.sort();
        expected_txids.sort();
        assert_eq!(txids, expected_txids);
    }
}

#[test]
fn test_conflicting_descendants() {
    let previous_output = OutPoint::new(h!("op"), 2);

    // tx_a spends previous_output
    let tx_a = Transaction {
        input: vec![TxIn {
            previous_output,
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(0)
    };

    // tx_a2 spends previous_output and conflicts with tx_a
    let tx_a2 = Transaction {
        input: vec![TxIn {
            previous_output,
            ..TxIn::default()
        }],
        output: vec![TxOut::default(), TxOut::default()],
        ..common::new_tx(1)
    };

    // tx_b spends tx_a
    let tx_b = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(2)
    };

    let txid_a = tx_a.txid();
    let txid_b = tx_b.txid();

    let mut graph = TxGraph::<()>::default();
    let _ = graph.insert_tx(tx_a);
    let _ = graph.insert_tx(tx_b);

    assert_eq!(
        graph
            .walk_conflicts(&tx_a2, 25, |depth, txid| Some((depth, txid)))
            .collect::<Vec<_>>(),
        vec![(0_usize, txid_a), (1_usize, txid_b),],
    );
}

#[test]
fn test_descendants_no_repeat() {
    let tx_a = Transaction {
        output: vec![TxOut::default(), TxOut::default(), TxOut::default()],
        ..common::new_tx(0)
    };

    let txs_b = (0..3)
        .map(|vout| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(tx_a.txid(), vout),
                ..TxIn::default()
            }],
            output: vec![TxOut::default()],
            ..common::new_tx(1)
        })
        .collect::<Vec<_>>();

    let txs_c = (0..2)
        .map(|vout| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(txs_b[vout as usize].txid(), vout),
                ..TxIn::default()
            }],
            output: vec![TxOut::default()],
            ..common::new_tx(2)
        })
        .collect::<Vec<_>>();

    let tx_d = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(txs_c[0].txid(), 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(txs_c[1].txid(), 0),
                ..TxIn::default()
            },
        ],
        output: vec![TxOut::default()],
        ..common::new_tx(3)
    };

    let tx_e = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_d.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![TxOut::default()],
        ..common::new_tx(4)
    };

    let txs_not_connected = (10..20)
        .map(|v| Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(h!("tx_does_not_exist"), v),
                ..TxIn::default()
            }],
            output: vec![TxOut::default()],
            ..common::new_tx(v)
        })
        .collect::<Vec<_>>();

    let mut graph = TxGraph::<()>::default();
    let mut expected_txids = BTreeSet::new();

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
        assert!(expected_txids.insert(tx.txid()));
    }

    let descendants = graph
        .walk_descendants(tx_a.txid(), |_, txid| Some(txid))
        .collect::<Vec<_>>();

    assert_eq!(descendants.len(), expected_txids.len());

    for txid in descendants {
        assert!(expected_txids.remove(&txid));
    }
    assert!(expected_txids.is_empty());
}

#[test]
fn test_chain_spends() {
    let local_chain: LocalChain = (0..=100)
        .map(|ht| (ht, BlockHash::hash(format!("Block Hash {}", ht).as_bytes())))
        .collect::<BTreeMap<u32, BlockHash>>()
        .into();
    let tip = local_chain.tip().expect("must have tip");

    // The parent tx contains 2 outputs. Which are spent by one confirmed and one unconfirmed tx.
    // The parent tx is confirmed at block 95.
    let tx_0 = Transaction {
        input: vec![],
        output: vec![
            TxOut {
                value: 10_000,
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: 20_000,
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..common::new_tx(0)
    };

    // The first confirmed transaction spends vout: 0. And is confirmed at block 98.
    let tx_1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.txid(), 0),
            ..TxIn::default()
        }],
        output: vec![
            TxOut {
                value: 5_000,
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: 5_000,
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..common::new_tx(0)
    };

    // The second transactions spends vout:1, and is unconfirmed.
    let tx_2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.txid(), 1),
            ..TxIn::default()
        }],
        output: vec![
            TxOut {
                value: 10_000,
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: 10_000,
                script_pubkey: ScriptBuf::new(),
            },
        ],
        ..common::new_tx(0)
    };

    let mut graph = TxGraph::<ConfirmationHeightAnchor>::default();

    let _ = graph.insert_tx(tx_0.clone());
    let _ = graph.insert_tx(tx_1.clone());
    let _ = graph.insert_tx(tx_2.clone());

    [95, 98]
        .iter()
        .zip([&tx_0, &tx_1].into_iter())
        .for_each(|(ht, tx)| {
            let _ = graph.insert_anchor(
                tx.txid(),
                ConfirmationHeightAnchor {
                    anchor_block: tip.block_id(),
                    confirmation_height: *ht,
                },
            );
        });

    // Assert that confirmed spends are returned correctly.
    assert_eq!(
        graph.get_chain_spend(&local_chain, tip.block_id(), OutPoint::new(tx_0.txid(), 0)),
        Some((
            ChainPosition::Confirmed(&ConfirmationHeightAnchor {
                anchor_block: tip.block_id(),
                confirmation_height: 98
            }),
            tx_1.txid(),
        )),
    );

    // Check if chain position is returned correctly.
    assert_eq!(
        graph.get_chain_position(&local_chain, tip.block_id(), tx_0.txid()),
        // Some(ObservedAs::Confirmed(&local_chain.get_block(95).expect("block expected"))),
        Some(ChainPosition::Confirmed(&ConfirmationHeightAnchor {
            anchor_block: tip.block_id(),
            confirmation_height: 95
        }))
    );

    // Even if unconfirmed tx has a last_seen of 0, it can still be part of a chain spend.
    assert_eq!(
        graph.get_chain_spend(&local_chain, tip.block_id(), OutPoint::new(tx_0.txid(), 1)),
        Some((ChainPosition::Unconfirmed(0), tx_2.txid())),
    );

    // Mark the unconfirmed as seen and check correct ObservedAs status is returned.
    let _ = graph.insert_seen_at(tx_2.txid(), 1234567);

    // Check chain spend returned correctly.
    assert_eq!(
        graph
            .get_chain_spend(&local_chain, tip.block_id(), OutPoint::new(tx_0.txid(), 1))
            .unwrap(),
        (ChainPosition::Unconfirmed(1234567), tx_2.txid())
    );

    // A conflicting transaction that conflicts with tx_1.
    let tx_1_conflict = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.txid(), 0),
            ..Default::default()
        }],
        ..common::new_tx(0)
    };
    let _ = graph.insert_tx(tx_1_conflict.clone());

    // Because this tx conflicts with an already confirmed transaction, chain position should return none.
    assert!(graph
        .get_chain_position(&local_chain, tip.block_id(), tx_1_conflict.txid())
        .is_none());

    // Another conflicting tx that conflicts with tx_2.
    let tx_2_conflict = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_0.txid(), 1),
            ..Default::default()
        }],
        ..common::new_tx(0)
    };

    // Insert in graph and mark it as seen.
    let _ = graph.insert_tx(tx_2_conflict.clone());
    let _ = graph.insert_seen_at(tx_2_conflict.txid(), 1234568);

    // This should return a valid observation with correct last seen.
    assert_eq!(
        graph
            .get_chain_position(&local_chain, tip.block_id(), tx_2_conflict.txid())
            .expect("position expected"),
        ChainPosition::Unconfirmed(1234568)
    );

    // Chain_spend now catches the new transaction as the spend.
    assert_eq!(
        graph
            .get_chain_spend(&local_chain, tip.block_id(), OutPoint::new(tx_0.txid(), 1))
            .expect("expect observation"),
        (ChainPosition::Unconfirmed(1234568), tx_2_conflict.txid())
    );

    // Chain position of the `tx_2` is now none, as it is older than `tx_2_conflict`
    assert!(graph
        .get_chain_position(&local_chain, tip.block_id(), tx_2.txid())
        .is_none());
}

/// Ensure that `last_seen` values only increase during [`Append::append`].
#[test]
fn test_changeset_last_seen_append() {
    let txid: Txid = h!("test txid");

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
        let update = ChangeSet::<()> {
            last_seen: update_ls.map(|ls| (txid, ls)).into_iter().collect(),
            ..Default::default()
        };

        original.append(update);
        assert_eq!(
            &original.last_seen.get(&txid).cloned(),
            Ord::max(original_ls, update_ls),
        );
    }
}

#[test]
fn test_missing_blocks() {
    /// An anchor implementation for testing, made up of `(the_anchor_block, random_data)`.
    #[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord, core::hash::Hash)]
    struct TestAnchor(BlockId);

    impl Anchor for TestAnchor {
        fn anchor_block(&self) -> BlockId {
            self.0
        }
    }

    struct Scenario<'a> {
        name: &'a str,
        graph: TxGraph<TestAnchor>,
        chain: LocalChain,
        exp_heights: &'a [u32],
    }

    const fn new_anchor(height: u32, hash: BlockHash) -> TestAnchor {
        TestAnchor(BlockId { height, hash })
    }

    fn new_scenario<'a>(
        name: &'a str,
        graph_anchors: &'a [(Txid, TestAnchor)],
        chain: &'a [(u32, BlockHash)],
        exp_heights: &'a [u32],
    ) -> Scenario<'a> {
        Scenario {
            name,
            graph: {
                let mut g = TxGraph::default();
                for (txid, anchor) in graph_anchors {
                    let _ = g.insert_anchor(*txid, anchor.clone());
                }
                g
            },
            chain: {
                let mut c = LocalChain::default();
                for (height, hash) in chain {
                    let _ = c.insert_block(BlockId {
                        height: *height,
                        hash: *hash,
                    });
                }
                c
            },
            exp_heights,
        }
    }

    fn run(scenarios: &[Scenario]) {
        for scenario in scenarios {
            let Scenario {
                name,
                graph,
                chain,
                exp_heights,
            } = scenario;

            let heights = graph.missing_heights(chain).collect::<Vec<_>>();
            assert_eq!(&heights, exp_heights, "scenario: {}", name);
        }
    }

    run(&[
        new_scenario(
            "2 txs with the same anchor (2:B) which is missing from chain",
            &[
                (h!("tx_1"), new_anchor(2, h!("B"))),
                (h!("tx_2"), new_anchor(2, h!("B"))),
            ],
            &[(1, h!("A")), (3, h!("C"))],
            &[2],
        ),
        new_scenario(
            "2 txs with different anchors at the same height, one of the anchors is missing",
            &[
                (h!("tx_1"), new_anchor(2, h!("B1"))),
                (h!("tx_2"), new_anchor(2, h!("B2"))),
            ],
            &[(1, h!("A")), (2, h!("B1"))],
            &[],
        ),
        new_scenario(
            "tx with 2 anchors of same height which are missing from the chain",
            &[
                (h!("tx"), new_anchor(3, h!("C1"))),
                (h!("tx"), new_anchor(3, h!("C2"))),
            ],
            &[(1, h!("A")), (4, h!("D"))],
            &[3],
        ),
        new_scenario(
            "tx with 2 anchors at the same height, chain has this height but does not match either anchor",
            &[
                (h!("tx"), new_anchor(4, h!("D1"))),
                (h!("tx"), new_anchor(4, h!("D2"))),
            ],
            &[(4, h!("D3")), (5, h!("E"))],
            &[],
        ),
        new_scenario(
            "tx with 2 anchors at different heights, one anchor exists in chain, should return nothing",
            &[
                (h!("tx"), new_anchor(3, h!("C"))),
                (h!("tx"), new_anchor(4, h!("D"))),
            ],
            &[(4, h!("D")), (5, h!("E"))],
            &[],
        ),
        new_scenario(
            "tx with 2 anchors at different heights, first height is already in chain with different hash, iterator should only return 2nd height",
            &[
                (h!("tx"), new_anchor(5, h!("E1"))),
                (h!("tx"), new_anchor(6, h!("F1"))),
            ],
            &[(4, h!("D")), (5, h!("E")), (7, h!("G"))],
            &[6],
        ),
        new_scenario(
            "tx with 2 anchors at different heights, neither height is in chain, both heights should be returned",
            &[
                (h!("tx"), new_anchor(3, h!("C"))),
                (h!("tx"), new_anchor(4, h!("D"))),
            ],
            &[(1, h!("A")), (2, h!("B"))],
            &[3, 4],
        ),
    ]);
}
