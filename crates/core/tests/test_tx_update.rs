use bdk_core::TxUpdate;

use bitcoin::{absolute::LockTime, transaction::Version, Transaction, Txid};
use bitcoin::hashes::Hash;
use std::sync::Arc;

#[test]
fn test_empty() {
    assert!(
        TxUpdate::<()>::default().is_empty(),
        "Default `TxUpdate` must be empty"
    );
}

#[test]
fn test_not_empty_when_txs_present() {
    let mut u = TxUpdate::<()>::default();

    let tx = Arc::new(Transaction {
        version: Version::ONE,
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![],
    });

    u.txs.push(tx);

    assert!(
        !u.is_empty(),
        "`TxUpdate` must not be empty when `txs` is populated"
    );
}

#[test]
fn test_not_empty_when_seen_ats_present() {
    let mut u = TxUpdate::<()>::default();

    let txid = Txid::from_byte_array([1u8; 32]);
    u.seen_ats.insert((txid, 123));

    assert!(
        !u.is_empty(),
        "`TxUpdate` must not be empty when `seen_ats` is populated"
    );
}

#[test]
fn test_extend_makes_update_non_empty() {
    let mut a = TxUpdate::<()>::default();
    let mut b = TxUpdate::<()>::default();

    let txid = Txid::from_byte_array([2u8; 32]);
    b.seen_ats.insert((txid, 999));

    a.extend(b);

    assert!(
        !a.is_empty(),
        "`TxUpdate` must not be empty after extending with a non-empty update"
    );
}

#[test]
fn test_map_anchors_transforms_anchor_type_and_preserves_txid() {
    let txid = Txid::from_byte_array([3u8; 32]);

    let mut u = TxUpdate::<u32>::default();
    u.anchors.insert((42u32, txid));
    u.seen_ats.insert((txid, 777));

    let mapped: TxUpdate<u64> = u.map_anchors(|a| (a as u64) + 1000);

    assert!(
        mapped.anchors.contains(&(1042u64, txid)),
        "mapped anchors must contain transformed anchor with the same txid"
    );

    assert!(
        mapped.seen_ats.contains(&(txid, 777)),
        "`seen_ats` should be preserved by map_anchors"
    );
}
