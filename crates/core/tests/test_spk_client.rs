use bdk_chain::local_chain::CheckPoint;
use bdk_core::spk_client::{FullScanResponse, SyncResponse};

use bitcoin::hashes::Hash;
use bitcoin::BlockHash;

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
fn test_sync_response_not_empty_when_chain_update_present() {
    let mut resp = SyncResponse::<()>::default();

    let bh = BlockHash::from_byte_array([0u8; 32]);
    resp.chain_update = Some(CheckPoint::new(0, bh));

    assert!(
        !resp.is_empty(),
        "SyncResponse must not be empty when chain_update is present"
    );
}

#[test]
fn test_full_scan_response_not_empty_when_last_active_indices_present() {
    let mut resp = FullScanResponse::<(), ()>::default();
    resp.last_active_indices.insert((), 0);

    assert!(
        !resp.is_empty(),
        "FullScanResponse must not be empty when last_active_indices is present"
    );
}
