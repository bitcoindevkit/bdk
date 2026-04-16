use bdk_core::bitcoin::hashes::Hash;
use bdk_core::bitcoin::{OutPoint, ScriptBuf, Txid};
use bdk_core::spk_client::{
    FullScanResponse, ScanItem, ScanProgress, ScanRequest, ScanResponse, SyncResponse,
};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObservedItem {
    Discovery(u32, u32, ScriptBuf),
    Spk(u32, ScriptBuf),
    Txid(Txid),
    OutPoint(OutPoint),
}

impl From<ScanItem<'_, u32, u32>> for ObservedItem {
    fn from(item: ScanItem<'_, u32, u32>) -> Self {
        match item {
            ScanItem::Discovery(keychain, index, spk) => {
                Self::Discovery(keychain, index, spk.to_owned())
            }
            ScanItem::Spk(index, spk) => Self::Spk(index, spk.to_owned()),
            ScanItem::Txid(txid) => Self::Txid(txid),
            ScanItem::OutPoint(outpoint) => Self::OutPoint(outpoint),
        }
    }
}

type ObservedLog = Arc<Mutex<Vec<(ObservedItem, ScanProgress)>>>;

/// Build a shared event log and the matching `inspect` closure for `ScanRequestBuilder`.
fn recorder() -> (
    ObservedLog,
    impl FnMut(ScanItem<'_, u32, u32>, ScanProgress) + Send + 'static,
) {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&observed);
    let inspect = move |item: ScanItem<'_, u32, u32>, progress| {
        captured.lock().unwrap().push((item.into(), progress));
    };
    (observed, inspect)
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
    assert!(
        ScanResponse::<(), ()>::default().is_empty(),
        "Default `ScanResponse` must be empty"
    );
}

#[test]
fn test_scan_request_with_explicit_items() {
    let spks = [
        ScriptBuf::from_bytes(vec![1, 2, 3]),
        ScriptBuf::from_bytes(vec![4, 5, 6]),
    ];
    let txid = Txid::from_byte_array([0xaa; 32]);
    let outpoint = OutPoint::new(txid, 0);

    let mut scan: ScanRequest = ScanRequest::builder()
        .spks(spks.clone())
        .txids([txid])
        .outpoints([outpoint])
        .build();

    assert!(scan.keychains().is_empty());
    assert_eq!(scan.stop_gap(), 20);

    let progress = scan.progress();
    assert_eq!(progress.explicit_total(), spks.len() + 2);
    assert_eq!(progress.explicit_consumed(), 0);
    assert_eq!(progress.discovery_consumed(), 0);

    assert_eq!(scan.next_spk_with_expected_txids().unwrap().spk, spks[0]);
    assert_eq!(scan.next_spk_with_expected_txids().unwrap().spk, spks[1]);
    assert!(scan.next_spk_with_expected_txids().is_none());

    assert_eq!(scan.next_txid().unwrap(), txid);
    assert!(scan.next_txid().is_none());

    assert_eq!(scan.next_outpoint().unwrap(), outpoint);
    assert!(scan.next_outpoint().is_none());

    let progress = scan.progress();
    assert_eq!(progress.explicit_consumed(), spks.len() + 2);
    assert_eq!(progress.explicit_remaining(), 0);
}

#[test]
fn test_scan_request_with_discovery() {
    let discovery_spks: Vec<(u32, ScriptBuf)> = (0u32..5)
        .map(|i| (i, ScriptBuf::from_bytes(vec![i as u8])))
        .collect();

    let mut scan = ScanRequest::<u32>::builder()
        .stop_gap(3)
        .discover_keychain(0, discovery_spks.clone())
        .build();

    assert_eq!(scan.stop_gap(), 3);
    assert_eq!(scan.keychains(), vec![0]);
    assert_eq!(scan.progress().explicit_total(), 0);
    assert_eq!(scan.progress().discovery_consumed(), 0);

    let discovered: Vec<_> = scan.iter_discovery_spks(0).collect();
    assert_eq!(discovered, discovery_spks);

    assert_eq!(scan.progress().discovery_consumed(), discovery_spks.len());
}

#[test]
fn test_scan_inspect_explicit_items() {
    let spks = [
        (10, ScriptBuf::from_bytes(vec![1, 2, 3])),
        (11, ScriptBuf::from_bytes(vec![4, 5, 6])),
    ];
    let txid = Txid::from_byte_array([0xaa; 32]);
    let outpoint = OutPoint::new(txid, 1);
    let (observed, inspect) = recorder();

    let mut scan = ScanRequest::<u32, u32>::builder()
        .spks_with_indexes(spks.clone())
        .txids([txid])
        .outpoints([outpoint])
        .inspect(inspect)
        .build();

    assert_eq!(scan.next_spk_with_expected_txids().unwrap().spk, spks[0].1);
    assert_eq!(scan.next_spk_with_expected_txids().unwrap().spk, spks[1].1);
    assert_eq!(scan.next_txid().unwrap(), txid);
    assert_eq!(scan.next_outpoint().unwrap(), outpoint);

    assert_eq!(
        *observed.lock().unwrap(),
        vec![
            (
                ObservedItem::Spk(spks[0].0, spks[0].1.clone()),
                ScanProgress {
                    spks_consumed: 1,
                    spks_remaining: 1,
                    txids_remaining: 1,
                    outpoints_remaining: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Spk(spks[1].0, spks[1].1.clone()),
                ScanProgress {
                    spks_consumed: 2,
                    txids_remaining: 1,
                    outpoints_remaining: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Txid(txid),
                ScanProgress {
                    spks_consumed: 2,
                    txids_consumed: 1,
                    outpoints_remaining: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::OutPoint(outpoint),
                ScanProgress {
                    spks_consumed: 2,
                    txids_consumed: 1,
                    outpoints_consumed: 1,
                    ..Default::default()
                },
            ),
        ]
    );
}

#[test]
fn test_scan_inspect_discovery_items() {
    let discovery_spks = [
        (0, ScriptBuf::from_bytes(vec![0])),
        (1, ScriptBuf::from_bytes(vec![1])),
        (2, ScriptBuf::from_bytes(vec![2])),
    ];
    let (observed, inspect) = recorder();

    let mut scan = ScanRequest::<u32, u32>::builder()
        .stop_gap(3)
        .discover_keychain(7, discovery_spks.clone())
        .inspect(inspect)
        .build();

    for (expected_index, expected_spk) in &discovery_spks {
        let (index, spk) = scan.next_discovery_spk(7).unwrap();
        assert_eq!(index, *expected_index);
        assert_eq!(spk, *expected_spk);
    }

    assert_eq!(
        *observed.lock().unwrap(),
        vec![
            (
                ObservedItem::Discovery(7, 0, discovery_spks[0].1.clone()),
                ScanProgress {
                    discovery_consumed: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Discovery(7, 1, discovery_spks[1].1.clone()),
                ScanProgress {
                    discovery_consumed: 2,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Discovery(7, 2, discovery_spks[2].1.clone()),
                ScanProgress {
                    discovery_consumed: 3,
                    ..Default::default()
                },
            ),
        ]
    );
}

#[test]
fn test_scan_inspect_mixed_request() {
    let discovery_spks = [
        (0, ScriptBuf::from_bytes(vec![0])),
        (1, ScriptBuf::from_bytes(vec![1])),
    ];
    let explicit_spk = (42, ScriptBuf::from_bytes(vec![9, 9, 9]));
    let txid = Txid::from_byte_array([0xbb; 32]);
    let (observed, inspect) = recorder();

    let mut scan = ScanRequest::<u32, u32>::builder()
        .stop_gap(2)
        .discover_keychain(7, discovery_spks.clone())
        .spks_with_indexes([explicit_spk.clone()])
        .txids([txid])
        .inspect(inspect)
        .build();

    assert_eq!(scan.next_discovery_spk(7).unwrap(), discovery_spks[0]);
    assert_eq!(
        scan.next_spk_with_expected_txids().unwrap().spk,
        explicit_spk.1
    );
    assert_eq!(scan.next_txid().unwrap(), txid);
    assert_eq!(scan.next_discovery_spk(7).unwrap(), discovery_spks[1]);

    assert_eq!(
        *observed.lock().unwrap(),
        vec![
            (
                ObservedItem::Discovery(7, 0, discovery_spks[0].1.clone()),
                ScanProgress {
                    discovery_consumed: 1,
                    spks_remaining: 1,
                    txids_remaining: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Spk(explicit_spk.0, explicit_spk.1.clone()),
                ScanProgress {
                    discovery_consumed: 1,
                    spks_consumed: 1,
                    txids_remaining: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Txid(txid),
                ScanProgress {
                    discovery_consumed: 1,
                    spks_consumed: 1,
                    txids_consumed: 1,
                    ..Default::default()
                },
            ),
            (
                ObservedItem::Discovery(7, 1, discovery_spks[1].1.clone()),
                ScanProgress {
                    discovery_consumed: 2,
                    spks_consumed: 1,
                    txids_consumed: 1,
                    ..Default::default()
                },
            ),
        ]
    );
}
