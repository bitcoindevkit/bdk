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
