use bdk_core::{collections::BTreeSet, BlockId, TxUpdate};
use bitcoin::{
    absolute, hashes::Hash, transaction, Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxOut,
};
use std::sync::Arc;

#[test]
fn test_empty() {
    assert!(
        TxUpdate::<()>::default().is_empty(),
        "Default `TxUpdate` must be empty"
    );
}

#[test]
fn test_is_empty_false_when_any_field_populated() {
    let tx = Arc::new(Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    });
    let txid = tx.compute_txid();
    let op = OutPoint::new(txid, 0);
    let txout = TxOut {
        value: Amount::from_sat(1_000),
        script_pubkey: ScriptBuf::new(),
    };

    // 1. txs
    let mut update = TxUpdate::<()>::default();
    update.txs.push(tx);
    assert!(!update.is_empty(), "TxUpdate with `txs` must not be empty");

    // 2. txouts
    let mut update = TxUpdate::<()>::default();
    update.txouts.insert(op, txout);
    assert!(
        !update.is_empty(),
        "TxUpdate with `txouts` must not be empty"
    );

    // 3. anchors
    let mut update = TxUpdate::<()>::default();
    update.anchors.insert(((), txid));
    assert!(
        !update.is_empty(),
        "TxUpdate with `anchors` must not be empty"
    );

    // 4. seen_ats
    let mut update = TxUpdate::<()>::default();
    update.seen_ats.insert((txid, 100));
    assert!(
        !update.is_empty(),
        "TxUpdate with `seen_ats` must not be empty"
    );

    // 5. evicted_ats
    let mut update = TxUpdate::<()>::default();
    update.evicted_ats.insert((txid, 200));
    assert!(
        !update.is_empty(),
        "TxUpdate with `evicted_ats` must not be empty"
    );
}

#[test]
fn test_extend() {
    let tx1 = Arc::new(Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    });
    let tx1_id = tx1.compute_txid();

    let tx2 = Arc::new(Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    });
    let tx2_id = tx2.compute_txid();

    let op1 = OutPoint::new(tx1_id, 0);
    let op2 = OutPoint::new(tx2_id, 0);

    let txout1 = TxOut {
        value: Amount::from_sat(10_000),
        script_pubkey: ScriptBuf::new(),
    };
    let txout2 = TxOut {
        value: Amount::from_sat(20_000),
        script_pubkey: ScriptBuf::new(),
    };
    let txout1_overwrite = TxOut {
        value: Amount::from_sat(15_000),
        script_pubkey: ScriptBuf::new(),
    };

    let mut update_a = TxUpdate::<u32>::default();
    update_a.txs.push(Arc::clone(&tx1));
    update_a.txouts.insert(op1, txout1);
    update_a.anchors.insert((10, tx1_id));
    update_a.seen_ats.insert((tx1_id, 100));
    update_a.evicted_ats.insert((tx1_id, 200));

    // Extending with an empty update leaves update_a unchanged
    let mut update_a_clone = update_a.clone();
    update_a_clone.extend(TxUpdate::default());
    assert_eq!(update_a_clone.txs.len(), 1);
    assert_eq!(update_a_clone.txouts.len(), 1);
    assert_eq!(update_a_clone.anchors.len(), 1);
    assert_eq!(update_a_clone.seen_ats.len(), 1);
    assert_eq!(update_a_clone.evicted_ats.len(), 1);

    // Extending an empty update with update_a populates all fields
    let mut empty_update = TxUpdate::<u32>::default();
    empty_update.extend(update_a.clone());
    assert_eq!(empty_update.txs, update_a.txs);
    assert_eq!(empty_update.txouts, update_a.txouts);
    assert_eq!(empty_update.anchors, update_a.anchors);
    assert_eq!(empty_update.seen_ats, update_a.seen_ats);
    assert_eq!(empty_update.evicted_ats, update_a.evicted_ats);

    // Construct update_b with overlapping txout (op1) and new elements
    let mut update_b = TxUpdate::<u32>::default();
    update_b.txs.push(Arc::clone(&tx2));
    update_b.txouts.insert(op1, txout1_overwrite.clone());
    update_b.txouts.insert(op2, txout2.clone());
    update_b.anchors.insert((20, tx2_id));
    update_b.seen_ats.insert((tx2_id, 300));
    update_b.evicted_ats.insert((tx2_id, 400));

    update_a.extend(update_b);

    assert_eq!(update_a.txs, vec![tx1, tx2]);
    assert_eq!(update_a.txouts.get(&op1), Some(&txout1_overwrite));
    assert_eq!(update_a.txouts.get(&op2), Some(&txout2));
    assert_eq!(
        update_a.anchors,
        BTreeSet::from([(10, tx1_id), (20, tx2_id)])
    );
    assert!(update_a.seen_ats.contains(&(tx1_id, 100)));
    assert!(update_a.seen_ats.contains(&(tx2_id, 300)));
    assert!(update_a.evicted_ats.contains(&(tx1_id, 200)));
    assert!(update_a.evicted_ats.contains(&(tx2_id, 400)));
}

#[test]
fn test_map_anchors() {
    let tx = Arc::new(Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    });
    let txid = tx.compute_txid();
    let op = OutPoint::new(txid, 0);
    let txout = TxOut {
        value: Amount::from_sat(5_000),
        script_pubkey: ScriptBuf::new(),
    };

    let block_id_10 = BlockId {
        height: 10,
        hash: BlockHash::all_zeros(),
    };
    let block_id_20 = BlockId {
        height: 20,
        hash: BlockHash::all_zeros(),
    };

    let mut update = TxUpdate::<BlockId>::default();
    update.txs.push(Arc::clone(&tx));
    update.txouts.insert(op, txout.clone());
    update.anchors.insert((block_id_10, txid));
    update.anchors.insert((block_id_20, txid));
    update.seen_ats.insert((txid, 100));
    update.evicted_ats.insert((txid, 200));

    // Map BlockId to u32 height
    let mapped = update.map_anchors(|b| b.height);

    // Anchors must be transformed to the new type
    assert_eq!(mapped.anchors, BTreeSet::from([(10, txid), (20, txid)]));

    // All other fields must be preserved exactly
    assert_eq!(mapped.txs, vec![tx]);
    assert_eq!(mapped.txouts.get(&op), Some(&txout));
    assert!(mapped.seen_ats.contains(&(txid, 100)));
    assert!(mapped.evicted_ats.contains(&(txid, 200)));
}
