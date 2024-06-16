#![doc = include_str!("../README.md")]

//! # Primary Methods
//!
//! The two primary methods are [`EsploraExt::sync`] and [`EsploraExt::full_scan`].
//!
//! [`EsploraExt::sync`] is used to sync against a subset of wallet data. For example, transaction
//! histories of revealed and unused script from the external (public) keychain and/or statuses of
//! wallet-owned UTXOs and spending transactions from them. The policy of what to sync against can
//! be customized.
//!
//! [`EsploraExt::full_scan`] is designed to be used when importing or restoring a keychain where
//! the range of possibly used scripts is not known. In this case it is necessary to scan all
//! keychain scripts until a number (the `stop_gap`) of unused scripts is discovered.
//!
//! For a sync or full scan, the user receives relevant blockchain data and output updates for
//! [`bdk_chain`] .
//!
//! # Low-Level Methods
//!
//! [`EsploraExt::sync`] and [`EsploraExt::full_scan`] returns updates which are *complete* and can
//! be used directly to determine confirmation statuses of each transaction. This is because a
//! [`LocalChain`] update is contained in the returned update structures. However, sometimes the
//! caller wishes to use a custom [`ChainOracle`] implementation (something other than
//! [`LocalChain`]). The following methods ONLY returns an update [`TxGraph`]:
//!
//! * [`EsploraExt::fetch_txs_with_keychain_spks`]
//! * [`EsploraExt::fetch_txs_with_spks`]
//! * [`EsploraExt::fetch_txs_with_txids`]
//! * [`EsploraExt::fetch_txs_with_outpoints`]
//!
//! # Stop Gap
//!
//! Methods [`EsploraExt::full_scan`] and [`EsploraExt::fetch_txs_with_keychain_spks`] takes in a
//! `stop_gap` input which is defined as the maximum number of consecutive unused script pubkeys to
//! scan transactions for before stopping.
//!
//! For example, with a `stop_gap` of 3, `full_scan` will keep scanning until it encounters 3
//! consecutive script pubkeys with no associated transactions.
//!
//! This follows the same approach as other Bitcoin-related software,
//! such as [Electrum](https://electrum.readthedocs.io/en/latest/faq.html#what-is-the-gap-limit),
//! [BTCPay Server](https://docs.btcpayserver.org/FAQ/Wallet/#the-gap-limit-problem),
//! and [Sparrow](https://www.sparrowwallet.com/docs/faq.html#ive-restored-my-wallet-but-some-of-my-funds-are-missing).
//!
//! A `stop_gap` of 0 will be treated as a `stop_gap` of 1.
//!
//! # Async
//!
//! Just like how [`EsploraExt`] extends the functionality of an
//! [`esplora_client::BlockingClient`], [`EsploraExt`] is the async version which extends
//! [`esplora_client::AsyncClient`].
//!
//! [`TxGraph`]: bdk_chain::tx_graph::TxGraph
//! [`LocalChain`]: bdk_chain::local_chain::LocalChain
//! [`ChainOracle`]: bdk_chain::ChainOracle
//! [`example_esplora`]: https://github.com/bitcoindevkit/bdk/tree/master/example-crates/example_esplora

use bdk_chain::{BlockId, ConfirmationBlockTime};
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

fn anchor_from_status(status: &TxStatus) -> Option<ConfirmationBlockTime> {
    if let TxStatus {
        block_height: Some(height),
        block_hash: Some(hash),
        block_time: Some(time),
        ..
    } = status.clone()
    {
        Some(ConfirmationBlockTime {
            block_id: BlockId { height, hash },
            confirmation_time: time,
        })
    } else {
        None
    }
}
