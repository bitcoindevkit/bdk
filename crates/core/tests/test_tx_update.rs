use std::sync::Arc;

use bdk_core::{TxUpdate, TxUpdateCursor};
use bitcoin::{absolute::LockTime, transaction::Version, OutPoint, Transaction, TxOut};

fn dummy_tx() -> Arc<Transaction> {
    Arc::new(Transaction {
        version: Version::ONE,
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![],
    })
}

#[test]
fn test_empty() {
    assert!(
        TxUpdate::<()>::default().is_empty(),
        "Default `TxUpdate` must be empty"
    );
}

#[test]
fn drain_since_returns_only_new_data() {
    let tx1 = dummy_tx();
    let tx2 = dummy_tx();
    let id1 = tx1.compute_txid();
    let id2 = tx2.compute_txid();

    let mut update = TxUpdate::default();
    let mut cursor = TxUpdateCursor::new();

    update.txs.push(tx1.clone());
    update.seen_ats.insert((id1, 1));

    let d1 = update.drain_since(&mut cursor);
    assert_eq!(d1.txs.len(), 1);
    assert!(d1.seen_ats.contains(&(id1, 1)));
    assert!(update.is_empty());

    update.txs.push(tx2.clone());
    update.anchors.insert(((), id2));

    let d2 = update.drain_since(&mut cursor);
    assert_eq!(d2.txs.len(), 1);
    assert!(d2.anchors.contains(&((), id2)));
    assert!(update.is_empty());
}

#[test]
fn drain_since_partitions_match_extend_roundtrip() {
    let tx = dummy_tx();
    let id = tx.compute_txid();
    let op = OutPoint { txid: id, vout: 0 };

    let mut full = TxUpdate::<()>::default();
    full.txs.push(tx);
    full.txouts.insert(op, TxOut::NULL);
    full.seen_ats.insert((id, 42));
    full.evicted_ats.insert((id, 99));

    let mut working = full.clone();
    let mut cursor = TxUpdateCursor::new();
    let d1 = working.drain_since(&mut cursor);
    let d2 = working.drain_since(&mut cursor);

    assert!(working.is_empty());
    assert!(d2.is_empty());

    let mut reconstructed = TxUpdate::default();
    reconstructed.extend(d1);
    assert_eq!(reconstructed.txs.len(), full.txs.len());
    assert_eq!(reconstructed.txouts, full.txouts);
    assert_eq!(reconstructed.seen_ats, full.seen_ats);
    assert_eq!(reconstructed.evicted_ats, full.evicted_ats);
}

#[test]
fn final_response_empty_after_full_drain() {
    let tx = dummy_tx();
    let mut update = TxUpdate::<()>::default();
    update.txs.push(tx);

    let mut cursor = TxUpdateCursor::new();
    let _ = update.drain_since(&mut cursor);

    assert!(
        update.is_empty(),
        "final tx_update must be empty after full drain"
    );
}
