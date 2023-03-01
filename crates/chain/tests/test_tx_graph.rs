#[macro_use]
mod common;
use bdk_chain::{
    collections::*,
    tx_graph::{Additions, TxGraph},
};
use bitcoin::{hashes::Hash, OutPoint, PackedLockTime, Script, Transaction, TxIn, TxOut, Txid};
use core::iter;

#[test]
fn insert_txouts() {
    let original_ops = [
        (
            OutPoint::new(h!("tx1"), 1),
            TxOut {
                value: 10_000,
                script_pubkey: Script::new(),
            },
        ),
        (
            OutPoint::new(h!("tx1"), 2),
            TxOut {
                value: 20_000,
                script_pubkey: Script::new(),
            },
        ),
    ];

    let update_ops = [(
        OutPoint::new(h!("tx2"), 0),
        TxOut {
            value: 20_000,
            script_pubkey: Script::new(),
        },
    )];

    let mut graph = {
        let mut graph = TxGraph::<Transaction>::default();
        for (outpoint, txout) in &original_ops {
            assert_eq!(
                graph.insert_txout(*outpoint, txout.clone()),
                Additions {
                    txout: [(*outpoint, txout.clone())].into(),
                    ..Default::default()
                }
            );
        }
        graph
    };

    let update = {
        let mut graph = TxGraph::<Transaction>::default();
        for (outpoint, txout) in &update_ops {
            assert_eq!(
                graph.insert_txout(*outpoint, txout.clone()),
                Additions {
                    txout: [(*outpoint, txout.clone())].into(),
                    ..Default::default()
                }
            );
        }
        graph
    };

    let additions = graph.determine_additions(&update);

    assert_eq!(
        additions,
        Additions {
            tx: [].into(),
            txout: update_ops.into(),
        }
    );

    graph.apply_additions(additions);
    assert_eq!(graph.all_txouts().count(), 3);
    assert_eq!(graph.full_transactions().count(), 0);
    assert_eq!(graph.partial_transactions().count(), 2);
}

#[test]
fn insert_tx_graph_doesnt_count_coinbase_as_spent() {
    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![],
    };

    let mut graph = TxGraph::default();
    let _ = graph.insert_tx(tx);
    assert!(graph.outspends(OutPoint::null()).is_empty());
    assert!(graph.tx_outspends(Txid::all_zeros()).next().is_none());
}

#[test]
fn insert_tx_graph_keeps_track_of_spend() {
    let tx1 = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut::default()],
    };

    let op = OutPoint {
        txid: tx1.txid(),
        vout: 0,
    };

    let tx2 = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: op,
            ..Default::default()
        }],
        output: vec![],
    };

    let mut graph1 = TxGraph::default();
    let mut graph2 = TxGraph::default();

    // insert in different order
    let _ = graph1.insert_tx(tx1.clone());
    let _ = graph1.insert_tx(tx2.clone());

    let _ = graph2.insert_tx(tx2.clone());
    let _ = graph2.insert_tx(tx1.clone());

    assert_eq!(
        &*graph1.outspends(op),
        &iter::once(tx2.txid()).collect::<HashSet<_>>()
    );
    assert_eq!(graph2.outspends(op), graph1.outspends(op));
}

#[test]
fn insert_tx_can_retrieve_full_tx_from_graph() {
    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut::default()],
    };

    let mut graph = TxGraph::default();
    let _ = graph.insert_tx(tx.clone());
    assert_eq!(graph.get_tx(tx.txid()), Some(&tx));
}

#[test]
fn insert_tx_displaces_txouts() {
    let mut tx_graph = TxGraph::default();
    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 42_000,
            script_pubkey: Script::default(),
        }],
    };

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1337_000,
            script_pubkey: Script::default(),
        },
    );

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1_000_000_000,
            script_pubkey: Script::default(),
        },
    );

    let _additions = tx_graph.insert_tx(tx.clone());

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
    let mut tx_graph = TxGraph::default();
    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 42_000,
            script_pubkey: Script::default(),
        }],
    };

    let _additions = tx_graph.insert_tx(tx.clone());

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1337_000,
            script_pubkey: Script::default(),
        },
    );

    let _ = tx_graph.insert_txout(
        OutPoint {
            txid: tx.txid(),
            vout: 0,
        },
        TxOut {
            value: 1_000_000_000,
            script_pubkey: Script::default(),
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
    let mut graph = TxGraph::default();
    let intx1 = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 100,
            ..Default::default()
        }],
    };
    let intx2 = Transaction {
        version: 0x02,
        lock_time: PackedLockTime(0),
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
        lock_time: PackedLockTime(0),
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

    assert_eq!(graph.calculate_fee(&tx), Some(100));

    tx.input.remove(2);

    // fee would be negative
    assert_eq!(graph.calculate_fee(&tx), Some(-200));

    // If we have an unknown outpoint, fee should return None.
    tx.input.push(TxIn {
        previous_output: OutPoint {
            txid: h!("unknown_txid"),
            vout: 0,
        },
        ..Default::default()
    });
    assert_eq!(graph.calculate_fee(&tx), None);
}

#[test]
fn test_calculate_fee_on_coinbase() {
    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut::default()],
    };

    let graph = TxGraph::<Transaction>::default();

    assert_eq!(graph.calculate_fee(&tx), Some(0));
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

    let mut graph = TxGraph::default();
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

    let mut graph = TxGraph::default();
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
