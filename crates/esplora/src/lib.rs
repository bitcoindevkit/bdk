#![doc = include_str!("../README.md")]
use std::collections::BTreeMap;

use bdk_chain::{local_chain::CheckPoint, ConfirmationTimeAnchor};
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

pub(crate) fn confirmation_time_anchor_maker(
    tip: &CheckPoint,
) -> impl FnMut(&TxStatus) -> Option<ConfirmationTimeAnchor> {
    let cache = tip
        .iter()
        .take(10)
        .map(|cp| (cp.height(), cp))
        .collect::<BTreeMap<u32, CheckPoint>>();

    move |status| match (status.block_time, status.block_height) {
        (Some(confirmation_time), Some(confirmation_height)) => {
            let (_, anchor_cp) = cache.range(confirmation_height..).next()?;

            Some(ConfirmationTimeAnchor {
                anchor_block: anchor_cp.block_id(),
                confirmation_height,
                confirmation_time,
            })
        }
        _ => None,
    }
}
