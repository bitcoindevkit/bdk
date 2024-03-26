#![doc = include_str!("../README.md")]

//! This crate is used for updating structures of [`bdk_chain`] with data from an Esplora server.
//!
//! The two primary methods are [`EsploraExt::sync`] and [`EsploraExt::full_scan`]. In most cases
//! [`EsploraExt::sync`] is used to sync the transaction histories of scripts that the application
//! cares about, for example the scripts for all the receive addresses of a Wallet's keychain that it
//! has shown a user. [`EsploraExt::full_scan`] is meant to be used when importing or restoring a
//! keychain where the range of possibly used scripts is not known. In this case it is necessary to
//! scan all keychain scripts until a number (the "stop gap") of unused scripts is discovered. For a
//! sync or full scan the user receives relevant blockchain data and output updates for [`bdk_chain`]
//! via a new [`TxGraph`] to be appended to any existing [`TxGraph`] data.
//!
//! Refer to [`example_esplora`] for a complete example.
//!
//! [`TxGraph`]: bdk_chain::tx_graph::TxGraph
//! [`example_esplora`]: https://github.com/bitcoindevkit/bdk/tree/master/example-crates/example_esplora

use std::collections::BTreeMap;

use bdk_chain::{local_chain, BlockId, ConfirmationTimeHeightAnchor, TxGraph};
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

/// Update returns from a full scan.
pub struct FullScanUpdate<K> {
    /// The update to apply to the receiving [`LocalChain`](local_chain::LocalChain).
    pub local_chain: local_chain::Update,
    /// The update to apply to the receiving [`TxGraph`].
    pub tx_graph: TxGraph<ConfirmationTimeHeightAnchor>,
    /// Last active indices for the corresponding keychains (`K`).
    pub last_active_indices: BTreeMap<K, u32>,
}

/// Update returned from a sync.
pub struct SyncUpdate {
    /// The update to apply to the receiving [`LocalChain`](local_chain::LocalChain).
    pub local_chain: local_chain::Update,
    /// The update to apply to the receiving [`TxGraph`].
    pub tx_graph: TxGraph<ConfirmationTimeHeightAnchor>,
}
