//! This crate is used for updating structures of [`bdk_chain`] with data from an esplora server.
//!
//! The star of the show is the  [`EsploraExt::scan`] method which scans for relevant
//! blockchain data (via esplora) and outputs a [`KeychainScan`](bdk_chain::keychain::KeychainScan).

use bdk_chain::ConfirmationTime;
use esplora_client::TxStatus;

pub use esplora_client;

#[cfg(feature = "blocking")]
mod blocking_ext;
#[cfg(feature = "blocking")]
pub use blocking_ext::*;

#[cfg(feature = "async")]
mod async_ext;
#[cfg(feature = "async")]
pub use async_ext::*;

pub(crate) fn map_confirmation_time(
    tx_status: &TxStatus,
    height_at_start: u32,
) -> ConfirmationTime {
    match (tx_status.block_time, tx_status.block_height) {
        (Some(time), Some(height)) if height <= height_at_start => {
            ConfirmationTime::Confirmed { height, time }
        }
        _ => ConfirmationTime::Unconfirmed,
    }
}
