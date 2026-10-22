use bdk_core::spk_client::{
    FullScanResponse, SpkWithExpectedTxids, SyncItem, SyncProgress, SyncRequest, SyncResponse,
};
use bitcoin::{hashes::Hash, OutPoint, ScriptBuf, Txid};
use std::sync::{Arc, Mutex};

#[test]
fn test_sync_progress_derives() {
    let p1 = SyncProgress {
        spks_consumed: 1,
        spks_remaining: 2,
        txids_consumed: 3,
        txids_remaining: 4,
        outpoints_consumed: 5,
        outpoints_remaining: 6,
    };
    let p2 = p1; // Copy check: p1 is still usable after assignment
    assert_eq!(p1, p2);
    assert_eq!(p1.spks_consumed, 1);
    assert_eq!(p2.spks_consumed, 1);

    let p3 = SyncProgress {
        spks_consumed: 10,
        ..p1
    };
    assert_ne!(p1, p3);
}

#[test]
fn test_spk_with_expected_txids_derives() {
    let spk_a = ScriptBuf::from_bytes(vec![0x51]);
    let spk_b = ScriptBuf::from_bytes(vec![0x52]);
    let txid = txid_from_byte(7);

    let mut item1 = SpkWithExpectedTxids::from(spk_a.clone());
    item1.expected_txids.insert(txid);

    let mut item2 = SpkWithExpectedTxids::from(spk_a);
    item2.expected_txids.insert(txid);

    let item3 = SpkWithExpectedTxids::from(spk_b);

    assert_eq!(item1, item2);
    assert_ne!(item1, item3);
}

#[test]
fn test_empty() {
    assert!(
        FullScanResponse::<(), ()>::default().is_empty(),
        "Default `FullScanResponse` must be empty"
    );
    assert!(
        SyncResponse::<()>::default().is_empty(),
        "Default `SyncResponse` must be empty"
    );
    // Custom checkpoint data type `D` must also support `is_empty()`
    assert!(
        FullScanResponse::<(), (), ()>::default().is_empty(),
        "Default `FullScanResponse` with custom checkpoint type must be empty"
    );
    assert!(
        SyncResponse::<(), ()>::default().is_empty(),
        "Default `SyncResponse` with custom checkpoint type must be empty"
    );
}

#[test]
fn sync_request_reports_progress_to_inspect() {
    let spk = ScriptBuf::from_bytes(vec![0x51]);
    let txid = txid_from_byte(1);
    let outpoint = OutPoint::new(txid_from_byte(2), 3);
    let inspected = Arc::new(Mutex::new(Vec::new()));

    let mut request = SyncRequest::builder_at(0)
        .spks([spk.clone()])
        .txids([txid])
        .outpoints([outpoint])
        .inspect({
            let inspected = Arc::clone(&inspected);
            move |item, p| {
                let kind = match item {
                    SyncItem::Spk(..) => "spk",
                    SyncItem::Txid(_) => "txid",
                    SyncItem::OutPoint(_) => "outpoint",
                };
                inspected.lock().unwrap().push((
                    kind,
                    p.spks_consumed,
                    p.txids_consumed,
                    p.outpoints_consumed,
                ));
            }
        })
        .build();

    assert_eq!(
        request.next_spk_with_expected_txids().map(|s| s.spk),
        Some(spk)
    );
    assert_eq!(request.next_txid(), Some(txid));
    assert_eq!(request.next_outpoint(), Some(outpoint));

    // `inspect` sees progress *after* the item is consumed.
    assert_eq!(
        *inspected.lock().unwrap(),
        [("spk", 1, 0, 0), ("txid", 1, 1, 0), ("outpoint", 1, 1, 1),]
    );
}

#[test]
fn sync_request_returns_expected_txids_for_matching_spk() {
    let spk = ScriptBuf::from_bytes(vec![0x51]);
    let other_spk = ScriptBuf::from_bytes(vec![0x52]);
    let txid_a = txid_from_byte(42);
    let txid_b = txid_from_byte(43);

    let mut request = SyncRequest::builder_at(0)
        .spks([spk.clone(), other_spk.clone()])
        .expected_spk_txids([(spk.clone(), txid_a)])
        .expected_spk_txids([(spk.clone(), txid_b)])
        .build();

    let first = request.next_spk_with_expected_txids().unwrap();
    assert_eq!(first.spk, spk);
    assert_eq!(first.expected_txids, [txid_a, txid_b].into());

    let second = request.next_spk_with_expected_txids().unwrap();
    assert_eq!(second.spk, other_spk);
    assert!(second.expected_txids.is_empty());
}

fn txid_from_byte(byte: u8) -> Txid {
    Txid::from_byte_array([byte; 32])
}
