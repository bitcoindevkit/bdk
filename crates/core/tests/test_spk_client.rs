use bdk_core::bitcoin::hashes::Hash;
use bdk_core::bitcoin::{BlockHash, OutPoint, Txid};
use bdk_core::spk_client::{FullScanResponse, SyncRequest, SyncResponse};

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
fn test_spks_does_not_consume() {
    let spk1 = bitcoin::ScriptBuf::new();
    let spk2 = bitcoin::ScriptBuf::new();

    let request: SyncRequest<u32, BlockHash> = SyncRequest::builder()
        .spks_with_indexes(vec![(0u32, spk1), (1u32, spk2)])
        .build();

    assert_eq!(request.spks().count(), 2);
    assert_eq!(request.spks().count(), 2, "must not consume");
}

#[test]
fn test_txids_does_not_consume() {
    let txid1 = Txid::from_byte_array([0x01; 32]);

    let request: SyncRequest<(), BlockHash> = SyncRequest::builder().txids(vec![txid1]).build();

    assert_eq!(request.txids().count(), 1);
    assert_eq!(request.txids().count(), 1, "must not consume");
}

#[test]
fn test_outpoints_does_not_consume() {
    let outpoint1 = OutPoint::null();

    let request: SyncRequest<(), BlockHash> =
        SyncRequest::builder().outpoints(vec![outpoint1]).build();

    assert_eq!(request.outpoints().count(), 1);
    assert_eq!(request.outpoints().count(), 1, "must not consume");
}

#[test]
fn test_drain_txids_still_consumes_all_items() {
    let txid1 = Txid::from_byte_array([0x01; 32]);
    let txid2 = Txid::from_byte_array([0x02; 32]);

    let mut request: SyncRequest<(), BlockHash> =
        SyncRequest::builder().txids(vec![txid1, txid2]).build();

    assert_eq!(request.txids().count(), 2);

    let consumed: Vec<_> = request.drain_txids().collect();
    assert_eq!(
        consumed.len(),
        2,
        "drain_txids must still consume all items"
    );
}
