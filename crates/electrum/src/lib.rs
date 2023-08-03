//! This crate is used for updating structures of the [`bdk_chain`] crate with data from electrum.
//!
//! The star of the show is the [`ElectrumExt::scan`] method, which scans for relevant blockchain
//! data (via electrum) and outputs an [`ElectrumUpdate`].
//!
//! An [`ElectrumUpdate`] only includes `txid`s and no full transactions. The caller is responsible
//! for obtaining full transactions before applying. This can be done with
//! these steps:
//!
//! 1. Determine which full transactions are missing. The method [`missing_full_txs`] of
//! [`ElectrumUpdate`] can be used.
//!
//! 2. Obtaining the full transactions. To do this via electrum, the method
//! [`batch_transaction_get`] can be used.
//!
//! Refer to [`bdk_electrum_example`] for a complete example.
//!
//! [`ElectrumClient::scan`]: electrum_client::ElectrumClient::scan
//! [`missing_full_txs`]: ElectrumUpdate::missing_full_txs
//! [`batch_transaction_get`]: electrum_client::ElectrumApi::batch_transaction_get
//! [`bdk_electrum_example`]: https://github.com/LLFourn/bdk_core_staging/tree/master/bdk_electrum_example

#![warn(missing_docs)]

mod electrum_ext;
pub use bdk_chain;
pub use electrum_client;
pub use electrum_ext::*;
