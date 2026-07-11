use std::sync::{Arc, Mutex};

use bdk_core::spk_client::{
    FullScanRequest, FullScanRequestEvent, FullScanResponse, SyncRequest, SyncRequestEvent,
    SyncResponse,
};
use bitcoin::{BlockHash, ScriptBuf};

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
fn sync_on_event_fires_item_started() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let events2 = events.clone();

    let request = SyncRequest::builder_at(0)
        .spks([ScriptBuf::new(), ScriptBuf::new()])
        .on_event(move |event| {
            if let SyncRequestEvent::ItemStarted(_item, progress) = event {
                events2.lock().unwrap().push(progress.consumed());
            }
        })
        .build();

    let mut request = request;
    let _ = request.next_spk_with_expected_txids();
    let _ = request.next_spk_with_expected_txids();

    let recorded = events.lock().unwrap();
    assert_eq!(recorded.as_slice(), &[1, 2]);
}

#[test]
fn sync_inspect_sugar_fires_item_started() {
    let events = Arc::new(Mutex::new(0usize));
    let events2 = events.clone();

    let mut request = SyncRequest::builder_at(0)
        .spks([ScriptBuf::new()])
        .inspect(move |_, progress| {
            *events2.lock().unwrap() = progress.consumed();
        })
        .build();

    let _ = request.next_spk_with_expected_txids();
    assert_eq!(*events.lock().unwrap(), 1);
}

#[test]
fn full_scan_on_event_fires_spk_started() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let events2 = events.clone();

    let mut request: FullScanRequest<u8, BlockHash> = FullScanRequest::builder_at(0)
        .spks_for_keychain(0u8, [(0u32, ScriptBuf::new()), (1u32, ScriptBuf::new())])
        .on_event(move |event| {
            if let FullScanRequestEvent::SpkStarted {
                index, progress, ..
            } = event
            {
                events2
                    .lock()
                    .unwrap()
                    .push((index, progress.keychain_spks_consumed));
            }
        })
        .build();

    let _ = request.iter_spks(0).collect::<Vec<_>>();

    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0], (0, 1));
    assert_eq!(recorded[1], (1, 2));
}

#[test]
fn full_scan_inspect_sugar_fires_spk_started() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let events2 = events.clone();

    let mut request: FullScanRequest<u8, BlockHash> = FullScanRequest::builder_at(0)
        .spks_for_keychain(0u8, [(5u32, ScriptBuf::new())])
        .inspect(move |keychain, index, _| {
            events2.lock().unwrap().push((keychain, index));
        })
        .build();

    let _ = request.iter_spks(0).collect::<Vec<_>>();

    let recorded = events.lock().unwrap();
    assert_eq!(recorded.as_slice(), &[(0u8, 5u32)]);
}
