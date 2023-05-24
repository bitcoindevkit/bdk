#![doc = include_str!("../README.md")]
use bdk_chain::{BlockId, ConfirmationTimeAnchor};
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

pub(crate) fn map_confirmation_time_anchor(
    tx_status: &TxStatus,
    tip_at_start: BlockId,
) -> Option<ConfirmationTimeAnchor> {
    match (tx_status.block_time, tx_status.block_height) {
        (Some(confirmation_time), Some(confirmation_height)) => Some(ConfirmationTimeAnchor {
            anchor_block: tip_at_start,
            confirmation_height,
            confirmation_time,
        }),
        _ => None,
    }
}
