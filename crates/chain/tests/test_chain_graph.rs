#[macro_use]
mod common;

use bdk_chain::{
    chain_graph::*,
    collections::HashSet,
    sparse_chain,
    tx_graph::{self, TxGraph},
    BlockId, TxHeight,
};
use bitcoin::{OutPoint, PackedLockTime, Script, Sequence, Transaction, TxIn, TxOut, Witness};

#[test]
fn test_spent_by() {
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
    let tx3 = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(42),
        input: vec![TxIn {
            previous_output: op,
            ..Default::default()
        }],
        output: vec![],
    };

    let mut cg1 = ChainGraph::default();
    let _ = cg1
        .insert_tx(tx1, TxHeight::Unconfirmed)
        .expect("should insert");
    let mut cg2 = cg1.clone();
    let _ = cg1
        .insert_tx(tx2.clone(), TxHeight::Unconfirmed)
        .expect("should insert");
    let _ = cg2
        .insert_tx(tx3.clone(), TxHeight::Unconfirmed)
        .expect("should insert");

    assert_eq!(cg1.spent_by(op), Some((&TxHeight::Unconfirmed, tx2.txid())));
    assert_eq!(cg2.spent_by(op), Some((&TxHeight::Unconfirmed, tx3.txid())));
}

#[test]
fn update_evicts_conflicting_tx() {
    let cp_a = BlockId {
        height: 0,
        hash: h!("A"),
    };
    let cp_b = BlockId {
        height: 1,
        hash: h!("B"),
    };
    let cp_b2 = BlockId {
        height: 1,
        hash: h!("B'"),
    };

    let tx_a = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut::default()],
    };

    let tx_b = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 0),
            script_sig: Script::new(),
            sequence: Sequence::default(),
            witness: Witness::new(),
        }],
        output: vec![TxOut::default()],
    };

    let tx_b2 = Transaction {
        version: 0x02,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 0),
            script_sig: Script::new(),
            sequence: Sequence::default(),
            witness: Witness::new(),
        }],
        output: vec![TxOut::default(), TxOut::default()],
    };
    {
        let mut cg1 = {
            let mut cg = ChainGraph::default();
            let _ = cg.insert_checkpoint(cp_a).expect("should insert cp");
            let _ = cg
                .insert_tx(tx_a.clone(), TxHeight::Confirmed(0))
                .expect("should insert tx");
            let _ = cg
                .insert_tx(tx_b.clone(), TxHeight::Unconfirmed)
                .expect("should insert tx");
            cg
        };
        let cg2 = {
            let mut cg = ChainGraph::default();
            let _ = cg
                .insert_tx(tx_b2.clone(), TxHeight::Unconfirmed)
                .expect("should insert tx");
            cg
        };

        let changeset = ChangeSet::<TxHeight, Transaction> {
            chain: sparse_chain::ChangeSet {
                checkpoints: Default::default(),
                txids: [
                    (tx_b.txid(), None),
                    (tx_b2.txid(), Some(TxHeight::Unconfirmed)),
                ]
                .into(),
            },
            graph: tx_graph::Additions {
                tx: [tx_b2.clone()].into(),
                txout: [].into(),
            },
        };
        assert_eq!(
            cg1.determine_changeset(&cg2),
            Ok(changeset.clone()),
            "tx should be evicted from mempool"
        );

        cg1.apply_changeset(changeset);
    }

    {
        let cg1 = {
            let mut cg = ChainGraph::default();
            let _ = cg.insert_checkpoint(cp_a).expect("should insert cp");
            let _ = cg.insert_checkpoint(cp_b).expect("should insert cp");
            let _ = cg
                .insert_tx(tx_a.clone(), TxHeight::Confirmed(0))
                .expect("should insert tx");
            let _ = cg
                .insert_tx(tx_b.clone(), TxHeight::Confirmed(1))
                .expect("should insert tx");
            cg
        };
        let cg2 = {
            let mut cg = ChainGraph::default();
            let _ = cg
                .insert_tx(tx_b2.clone(), TxHeight::Unconfirmed)
                .expect("should insert tx");
            cg
        };
        assert_eq!(
            cg1.determine_changeset(&cg2),
            Err(UpdateError::UnresolvableConflict(UnresolvableConflict {
                already_confirmed_tx: (TxHeight::Confirmed(1), tx_b.txid()),
                update_tx: (TxHeight::Unconfirmed, tx_b2.txid()),
            })),
            "fail if tx is evicted from valid block"
        );
    }

    {
        // Given 2 blocks `{A, B}`, and an update that invalidates block B with
        // `{A, B'}`, we expect txs that exist in `B` that conflicts with txs
        // introduced in the update to be successfully evicted.
        let mut cg1 = {
            let mut cg = ChainGraph::default();
            let _ = cg.insert_checkpoint(cp_a).expect("should insert cp");
            let _ = cg.insert_checkpoint(cp_b).expect("should insert cp");
            let _ = cg
                .insert_tx(tx_a.clone(), TxHeight::Confirmed(0))
                .expect("should insert tx");
            let _ = cg
                .insert_tx(tx_b.clone(), TxHeight::Confirmed(1))
                .expect("should insert tx");
            cg
        };
        let cg2 = {
            let mut cg = ChainGraph::default();
            let _ = cg.insert_checkpoint(cp_a).expect("should insert cp");
            let _ = cg.insert_checkpoint(cp_b2).expect("should insert cp");
            let _ = cg
                .insert_tx(tx_b2.clone(), TxHeight::Unconfirmed)
                .expect("should insert tx");
            cg
        };

        let changeset = ChangeSet::<TxHeight, Transaction> {
            chain: sparse_chain::ChangeSet {
                checkpoints: [(1, Some(h!("B'")))].into(),
                txids: [
                    (tx_b.txid(), None),
                    (tx_b2.txid(), Some(TxHeight::Unconfirmed)),
                ]
                .into(),
            },
            graph: tx_graph::Additions {
                tx: [tx_b2.clone()].into(),
                txout: [].into(),
            },
        };
        assert_eq!(
            cg1.determine_changeset(&cg2),
            Ok(changeset.clone()),
            "tx should be evicted from B",
        );

        cg1.apply_changeset(changeset);
    }
}

#[test]
fn chain_graph_new_missing() {
    let tx_a = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut::default()],
    };
    let tx_b = Transaction {
        version: 0x02,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut::default()],
    };

    let update = chain!(
        index: TxHeight,
        checkpoints: [[0, h!("A")]],
        txids: [
            (tx_a.txid(), TxHeight::Confirmed(0)),
            (tx_b.txid(), TxHeight::Confirmed(0))
        ]
    );
    let mut graph = TxGraph::default();

    let mut expected_missing = HashSet::new();
    expected_missing.insert(tx_a.txid());
    expected_missing.insert(tx_b.txid());

    assert_eq!(
        ChainGraph::new(update.clone(), graph.clone()),
        Err(NewError::Missing(expected_missing.clone()))
    );

    let _ = graph.insert_tx(tx_b.clone());
    expected_missing.remove(&tx_b.txid());

    assert_eq!(
        ChainGraph::new(update.clone(), graph.clone()),
        Err(NewError::Missing(expected_missing.clone()))
    );

    let _ = graph.insert_txout(
        OutPoint {
            txid: tx_a.txid(),
            vout: 0,
        },
        tx_a.output[0].clone(),
    );

    assert_eq!(
        ChainGraph::new(update.clone(), graph.clone()),
        Err(NewError::Missing(expected_missing)),
        "inserting an output instead of full tx doesn't satisfy constraint"
    );

    let _ = graph.insert_tx(tx_a.clone());

    let new_graph = ChainGraph::new(update.clone(), graph.clone()).unwrap();
    let expected_graph = {
        let mut cg = ChainGraph::<TxHeight, Transaction>::default();
        let _ = cg
            .insert_checkpoint(update.latest_checkpoint().unwrap())
            .unwrap();
        let _ = cg.insert_tx(tx_a, TxHeight::Confirmed(0)).unwrap();
        let _ = cg.insert_tx(tx_b, TxHeight::Confirmed(0)).unwrap();
        cg
    };

    assert_eq!(new_graph, expected_graph);
}

#[test]
fn chain_graph_new_conflicts() {
    let tx_a = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut::default()],
    };

    let tx_b = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 0),
            script_sig: Script::new(),
            sequence: Sequence::default(),
            witness: Witness::new(),
        }],
        output: vec![TxOut::default()],
    };

    let tx_b2 = Transaction {
        version: 0x02,
        lock_time: PackedLockTime(0),
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 0),
            script_sig: Script::new(),
            sequence: Sequence::default(),
            witness: Witness::new(),
        }],
        output: vec![TxOut::default(), TxOut::default()],
    };

    let chain = chain!(
        index: TxHeight,
        checkpoints: [[5, h!("A")]],
        txids: [
            (tx_a.txid(), TxHeight::Confirmed(1)),
            (tx_b.txid(), TxHeight::Confirmed(2)),
            (tx_b2.txid(), TxHeight::Confirmed(3))
        ]
    );

    let graph = TxGraph::new([tx_a, tx_b, tx_b2]);

    assert!(matches!(
        ChainGraph::new(chain, graph),
        Err(NewError::Conflict { .. })
    ));
}

#[test]
fn test_get_tx_in_chain() {
    let mut cg = ChainGraph::default();
    let tx = Transaction {
        version: 0x01,
        lock_time: PackedLockTime(0),
        input: vec![],
        output: vec![TxOut::default()],
    };

    let _ = cg.insert_tx(tx.clone(), TxHeight::Unconfirmed).unwrap();
    assert_eq!(
        cg.get_tx_in_chain(tx.txid()),
        Some((&TxHeight::Unconfirmed, &tx))
    );
}

#[test]
fn test_iterate_transactions() {
    let mut cg = ChainGraph::default();
    let txs = (0..3)
        .map(|i| Transaction {
            version: i,
            lock_time: PackedLockTime(0),
            input: vec![],
            output: vec![TxOut::default()],
        })
        .collect::<Vec<_>>();
    let _ = cg
        .insert_checkpoint(BlockId {
            height: 1,
            hash: h!("A"),
        })
        .unwrap();
    let _ = cg
        .insert_tx(txs[0].clone(), TxHeight::Confirmed(1))
        .unwrap();
    let _ = cg.insert_tx(txs[1].clone(), TxHeight::Unconfirmed).unwrap();
    let _ = cg
        .insert_tx(txs[2].clone(), TxHeight::Confirmed(0))
        .unwrap();

    assert_eq!(
        cg.transactions_in_chain().collect::<Vec<_>>(),
        vec![
            (&TxHeight::Confirmed(0), &txs[2]),
            (&TxHeight::Confirmed(1), &txs[0]),
            (&TxHeight::Unconfirmed, &txs[1]),
        ]
    );
}

/// Start with: block1, block2a, tx1, tx2a
///   Update 1: block2a -> block2b , tx2a -> tx2b
///   Update 2: block2b -> block2c , tx2b -> tx2a
#[test]
fn test_apply_changes_reintroduce_tx() {
    let block1 = BlockId {
        height: 1,
        hash: h!("block 1"),
    };
    let block2a = BlockId {
        height: 2,
        hash: h!("block 2a"),
    };
    let block2b = BlockId {
        height: 2,
        hash: h!("block 2b"),
    };
    let block2c = BlockId {
        height: 2,
        hash: h!("block 2c"),
    };

    let tx1 = Transaction {
        version: 0,
        lock_time: PackedLockTime(1),
        input: Vec::new(),
        output: [TxOut {
            value: 1,
            script_pubkey: Script::new(),
        }]
        .into(),
    };

    let tx2a = Transaction {
        version: 0,
        lock_time: PackedLockTime('a'.into()),
        input: [TxIn {
            previous_output: OutPoint::new(tx1.txid(), 0),
            ..Default::default()
        }]
        .into(),
        output: [TxOut {
            value: 0,
            ..Default::default()
        }]
        .into(),
    };

    let tx2b = Transaction {
        lock_time: PackedLockTime('b'.into()),
        ..tx2a.clone()
    };

    // block1, block2a, tx1, tx2a
    let mut cg = {
        let mut cg = ChainGraph::default();
        let _ = cg.insert_checkpoint(block1).unwrap();
        let _ = cg.insert_checkpoint(block2a).unwrap();
        let _ = cg.insert_tx(tx1.clone(), TxHeight::Confirmed(1)).unwrap();
        let _ = cg.insert_tx(tx2a.clone(), TxHeight::Confirmed(2)).unwrap();
        cg
    };

    // block2a -> block2b , tx2a -> tx2b
    let update = {
        let mut update = ChainGraph::default();
        let _ = update.insert_checkpoint(block1).unwrap();
        let _ = update.insert_checkpoint(block2b).unwrap();
        let _ = update
            .insert_tx(tx2b.clone(), TxHeight::Confirmed(2))
            .unwrap();
        update
    };
    assert_eq!(
        cg.apply_update(update).expect("should update"),
        ChangeSet {
            chain: changeset! {
                checkpoints: [(2, Some(block2b.hash))],
                txids: [(tx2a.txid(), None), (tx2b.txid(), Some(TxHeight::Confirmed(2)))]
            },
            graph: tx_graph::Additions {
                tx: [tx2b.clone()].into(),
                ..Default::default()
            },
        }
    );

    // block2b -> block2c , tx2b -> tx2a
    let update = {
        let mut update = ChainGraph::default();
        let _ = update.insert_checkpoint(block1).unwrap();
        let _ = update.insert_checkpoint(block2c).unwrap();
        let _ = update
            .insert_tx(tx2a.clone(), TxHeight::Confirmed(2))
            .unwrap();
        update
    };
    assert_eq!(
        cg.apply_update(update).expect("should update"),
        ChangeSet {
            chain: changeset! {
                checkpoints: [(2, Some(block2c.hash))],
                txids: [(tx2b.txid(), None), (tx2a.txid(), Some(TxHeight::Confirmed(2)))]
            },
            ..Default::default()
        }
    );
}

#[test]
fn test_evict_descendants() {
    let block_1 = BlockId {
        height: 1,
        hash: h!("block 1"),
    };

    let block_2a = BlockId {
        height: 2,
        hash: h!("block 2 a"),
    };

    let block_2b = BlockId {
        height: 2,
        hash: h!("block 2 b"),
    };

    let tx_1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(h!("fake tx"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 10_000,
            script_pubkey: Script::new(),
        }],
        ..common::new_tx(1)
    };
    let tx_2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_1.txid(), 0),
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: 20_000,
                script_pubkey: Script::new(),
            },
            TxOut {
                value: 30_000,
                script_pubkey: Script::new(),
            },
        ],
        ..common::new_tx(2)
    };
    let tx_3 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_2.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 40_000,
            script_pubkey: Script::new(),
        }],
        ..common::new_tx(3)
    };
    let tx_4 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_2.txid(), 1),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 40_000,
            script_pubkey: Script::new(),
        }],
        ..common::new_tx(4)
    };
    let tx_5 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_4.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 40_000,
            script_pubkey: Script::new(),
        }],
        ..common::new_tx(5)
    };

    let tx_conflict = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_1.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 12345,
            script_pubkey: Script::new(),
        }],
        ..common::new_tx(6)
    };

    // 1 is spent by 2, 2 is spent by 3 and 4, 4 is spent by 5
    let _txid_1 = tx_1.txid();
    let txid_2 = tx_2.txid();
    let txid_3 = tx_3.txid();
    let txid_4 = tx_4.txid();
    let txid_5 = tx_5.txid();

    // this tx conflicts with 2
    let txid_conflict = tx_conflict.txid();

    let cg = {
        let mut cg = ChainGraph::<TxHeight>::default();
        let _ = cg.insert_checkpoint(block_1);
        let _ = cg.insert_checkpoint(block_2a);
        let _ = cg.insert_tx(tx_1, TxHeight::Confirmed(1));
        let _ = cg.insert_tx(tx_2, TxHeight::Confirmed(2));
        let _ = cg.insert_tx(tx_3, TxHeight::Confirmed(2));
        let _ = cg.insert_tx(tx_4, TxHeight::Confirmed(2));
        let _ = cg.insert_tx(tx_5, TxHeight::Confirmed(2));
        cg
    };

    let update = {
        let mut cg = ChainGraph::<TxHeight>::default();
        let _ = cg.insert_checkpoint(block_1);
        let _ = cg.insert_checkpoint(block_2b);
        let _ = cg.insert_tx(tx_conflict.clone(), TxHeight::Confirmed(2));
        cg
    };

    assert_eq!(
        cg.determine_changeset(&update),
        Ok(ChangeSet {
            chain: changeset! {
                checkpoints: [(2, Some(block_2b.hash))],
                txids: [(txid_2, None), (txid_3, None), (txid_4, None), (txid_5, None), (txid_conflict, Some(TxHeight::Confirmed(2)))]
            },
            graph: tx_graph::Additions {
                tx: [tx_conflict.clone()].into(),
                ..Default::default()
            }
        })
    );

    let err = cg
        .insert_tx_preview(tx_conflict.clone(), TxHeight::Unconfirmed)
        .expect_err("must fail due to conflicts");
    assert!(matches!(err, InsertTxError::UnresolvableConflict(_)));
}
