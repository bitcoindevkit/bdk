//! This crate is used for updating structures of [`bdk_chain`] with data from an Electrum server.
//!
//! The two primary methods are [`ElectrumExt::sync`] and [`ElectrumExt::full_scan`]. In most cases
//! [`ElectrumExt::sync`] is used to sync the transaction histories of scripts that the application
//! cares about, for example the scripts for all the receive addresses of a Wallet's keychain that it
//! has shown a user. [`ElectrumExt::full_scan`] is meant to be used when importing or restoring a
//! keychain where the range of possibly used scripts is not known. In this case it is necessary to
//! scan all keychain scripts until a number (the "stop gap") of unused scripts is discovered. For a
//! sync or full scan the user receives relevant blockchain data and output updates for
//! [`bdk_chain`] including [`RelevantTxids`].
//!
//! The [`RelevantTxids`] only includes `txid`s and not full transactions. The caller is responsible
//! for obtaining full transactions before applying new data to their [`bdk_chain`]. This can be
//! done with these steps:
//!
//! 1. Determine which full transactions are missing. Use [`RelevantTxids::missing_full_txs`].
//!
//! 2. Obtaining the full transactions. To do this via electrum use [`ElectrumApi::batch_transaction_get`].
//!
//! Refer to [`example_electrum`] for a complete example.
//!
//! [`ElectrumApi::batch_transaction_get`]: electrum_client::ElectrumApi::batch_transaction_get
//! [`example_electrum`]: https://github.com/bitcoindevkit/bdk/tree/master/example-crates/example_electrum

#![warn(missing_docs)]

mod electrum_ext;
pub use bdk_chain;
pub use electrum_client;
pub use electrum_ext::*;
