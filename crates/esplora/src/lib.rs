#![doc = include_str!("../README.md")]
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
