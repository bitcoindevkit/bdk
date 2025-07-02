//! This crate is used for returning updates from Electrum servers.
//!
//! Updates are returned as either a [`SyncResponse`] (if [`BdkElectrumClient::sync()`] is called),
//! or a [`FullScanResponse`] (if [`BdkElectrumClient::full_scan()`] is called).
//!
//! In most cases [`BdkElectrumClient::sync()`] is used to sync the transaction histories of scripts
//! that the application cares about, for example the scripts for all the receive addresses of a
//! Wallet's keychain that it has shown a user.
//!
//! [`BdkElectrumClient::full_scan`] is meant to be used when importing or restoring a keychain
//! where the range of possibly used scripts is not known. In this case it is necessary to scan all
//! keychain scripts until a number (the "stop gap") of unused scripts is discovered.
//!
//! Refer to [`example_electrum`] for a complete example.
//!
//! [`example_electrum`]: https://github.com/bitcoindevkit/bdk/tree/master/examples/example_electrum
//! [`SyncResponse`]: bdk_core::spk_client::SyncResponse
//! [`FullScanResponse`]: bdk_core::spk_client::FullScanResponse
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

mod bdk_electrum_client;
pub use bdk_electrum_client::*;

pub use bdk_core;
pub use electrum_client;
