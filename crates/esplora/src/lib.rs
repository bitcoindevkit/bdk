#![doc = include_str!("../README.md")]
use bdk_chain::{BlockId, ConfirmationTimeHeightAnchor};
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

const ASSUME_FINAL_DEPTH: u32 = 15;

fn anchor_from_status(status: &TxStatus) -> Option<ConfirmationTimeHeightAnchor> {
    if let TxStatus {
        block_height: Some(height),
        block_hash: Some(hash),
        block_time: Some(time),
        ..
    } = status.clone()
    {
        Some(ConfirmationTimeHeightAnchor {
            anchor_block: BlockId { height, hash },
            confirmation_height: height,
            confirmation_time: time,
        })
    } else {
        None
    }
}
