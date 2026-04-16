//! This crate is used for returning updates from Electrum servers.
//!
//! Updates are returned as a [`ScanResponse`] when [`BdkElectrumClient::scan()`] is called. The
//! older [`BdkElectrumClient::sync()`] and [`BdkElectrumClient::full_scan()`] methods remain
//! available, returning [`SyncResponse`] and [`FullScanResponse`] respectively.
//!
//! In most cases [`BdkElectrumClient::scan()`] should be used to combine keychain discovery with
//! syncing the transaction histories of scripts that the application cares about, for example the
//! scripts for all the receive addresses of a Wallet's keychain that it has shown a user.
//!
//! [`BdkElectrumClient::scan()`] is intended to replace the
//! [`BdkElectrumClient::full_scan()`] and [`BdkElectrumClient::sync()`] APIs.
//!
//! Refer to [`example_electrum`] for a complete example.
//!
//! [`example_electrum`]: https://github.com/bitcoindevkit/bdk/tree/master/examples/example_electrum
//! [`ScanResponse`]: bdk_core::spk_client::ScanResponse
//! [`SyncResponse`]: bdk_core::spk_client::SyncResponse
//! [`FullScanResponse`]: bdk_core::spk_client::FullScanResponse
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

mod bdk_electrum_client;
pub use bdk_electrum_client::*;

pub use bdk_core;
pub use electrum_client;
