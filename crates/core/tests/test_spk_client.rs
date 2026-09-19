use bdk_core::spk_client::{FullScanResponse, SyncProgress, SyncRequest, SyncResponse};
use bitcoin::{hashes::Hash, OutPoint, ScriptBuf, Txid};
use std::sync::{Arc, Mutex};

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
}

#[test]
fn sync_request_reports_progress_to_inspect() {
    let spk = ScriptBuf::from_bytes(vec![0x51]);
    let txid = txid_from_byte(1);
    let outpoint = OutPoint::new(txid_from_byte(2), 3);
    let inspected = Arc::new(Mutex::new(Vec::new()));

    let mut request = SyncRequest::builder_at(42)
        .spks([spk.clone()])
        .txids([txid])
        .outpoints([outpoint])
        .inspect({
            let inspected = Arc::clone(&inspected);
            move |item, progress| {
                inspected
                    .lock()
                    .expect("inspect log must not be poisoned")
                    .push((item.to_string(), progress));
            }
        })
        .build();

    assert_eq!(request.start_time(), 42);
    assert_eq!(request.progress().remaining(), 3);
    assert_eq!(
        request.next_spk_with_expected_txids().map(|s| s.spk),
        Some(spk)
    );
    assert_eq!(request.next_txid(), Some(txid));
    assert_eq!(request.next_outpoint(), Some(outpoint));
    assert_eq!(request.progress().remaining(), 0);

    let inspected = inspected.lock().expect("inspect log must not be poisoned");
    assert_eq!(inspected.len(), 3);
    assert_progress(&inspected[0].1, (1, 0), (0, 1), (0, 1));
    assert_progress(&inspected[1].1, (1, 0), (1, 0), (0, 1));
    assert_progress(&inspected[2].1, (1, 0), (1, 0), (1, 0));
    assert!(inspected[0].0.contains("script"));
    assert!(inspected[1].0.contains("txid"));
    assert!(inspected[2].0.contains("outpoint"));
}

#[test]
fn sync_request_returns_expected_txids_for_matching_spk() {
    let spk = ScriptBuf::from_bytes(vec![0x51]);
    let other_spk = ScriptBuf::from_bytes(vec![0x52]);
    let expected_txid = txid_from_byte(42);
    let unrelated_txid = txid_from_byte(99);

    let mut request = SyncRequest::builder_at(0)
        .spks([spk.clone(), other_spk])
        .expected_spk_txids([(spk.clone(), expected_txid)])
        .expected_spk_txids([(ScriptBuf::from_bytes(vec![0x53]), unrelated_txid)])
        .build();

    let first = request
        .next_spk_with_expected_txids()
        .expect("first spk must be present");
    assert_eq!(first.spk, spk);
    assert_eq!(first.expected_txids.len(), 1);
    assert!(first.expected_txids.contains(&expected_txid));

    let second = request
        .next_spk_with_expected_txids()
        .expect("second spk must be present");
    assert!(second.expected_txids.is_empty());
}

fn txid_from_byte(byte: u8) -> Txid {
    Txid::from_byte_array([byte; 32])
}

fn assert_progress(
    progress: &SyncProgress,
    spks: (usize, usize),
    txids: (usize, usize),
    outpoints: (usize, usize),
) {
    assert_eq!(
        (
            progress.spks_consumed,
            progress.spks_remaining,
            progress.txids_consumed,
            progress.txids_remaining,
            progress.outpoints_consumed,
            progress.outpoints_remaining,
        ),
        (spks.0, spks.1, txids.0, txids.1, outpoints.0, outpoints.1)
    );
}
