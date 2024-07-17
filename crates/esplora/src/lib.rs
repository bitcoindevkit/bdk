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

use bdk_chain::{bitcoin::Txid, Anchor, BlockId, BlockTime};
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

fn anchor_from_status(txid: Txid, status: &TxStatus) -> Option<(Anchor, BlockTime)> {
    if let TxStatus {
        block_height: Some(height),
        block_hash: Some(hash),
        block_time: Some(time),
        ..
    } = status.clone()
    {
        Some((
            (txid, BlockId { height, hash }),
            BlockTime::new(time as u32),
        ))
    } else {
        None
    }
}
