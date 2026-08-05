#![doc = include_str!("../README.md")]
//! # Stop Gap
//!
//! [`EsploraExt::full_scan`] takes in a `stop_gap` input which is defined as the maximum number of
//! consecutive unused script pubkeys to scan transactions for before stopping.
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
//! [`esplora_client::BlockingClient`], [`EsploraAsyncExt`] is the async version which extends
//! [`esplora_client::AsyncClient`].
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use bdk_core::bitcoin::{Amount, OutPoint, Transaction, TxOut, Txid};
use bdk_core::collections::HashMap;
use bdk_core::{BlockId, ConfirmationBlockTime, TxUpdate};
use esplora_client::TxStatus;
use std::sync::{Arc, Mutex};

pub use esplora_client;

#[cfg(feature = "blocking")]
mod blocking_ext;
#[cfg(feature = "blocking")]
pub use blocking_ext::*;

#[cfg(feature = "async")]
mod async_ext;
#[cfg(feature = "async")]
pub use async_ext::*;

/// Wrapper around an Esplora client (either [`esplora_client::BlockingClient`] or
/// [`esplora_client::AsyncClient`]) which maintains an internal in-memory transaction cache to
/// avoid re-fetching the full body of transactions that have already been downloaded in a
/// previous sync.
///
/// This mirrors the caching behavior of `bdk_electrum`'s `BdkElectrumClient`. For a `txid` that
/// is already present in the cache, only its confirmation status is re-checked (via
/// [`esplora_client::BlockingClient::get_tx_status`] / [`esplora_client::AsyncClient::get_tx_status`])
/// instead of re-downloading the full transaction body.
#[derive(Debug)]
pub struct BdkEsploraClient<C> {
    /// The internal esplora client.
    pub inner: C,
    /// The transaction cache.
    tx_cache: Mutex<HashMap<Txid, Arc<Transaction>>>,
}

impl<C> BdkEsploraClient<C> {
    /// Creates a new bdk client from an esplora client.
    pub fn new(client: C) -> Self {
        Self {
            inner: client,
            tx_cache: Default::default(),
        }
    }

    /// Insert transactions into the transaction cache so that the client will not re-fetch them.
    ///
    /// Typically used to pre-populate the cache from an existing `TxGraph`.
    pub fn populate_tx_cache(&self, txs: impl IntoIterator<Item = impl Into<Arc<Transaction>>>) {
        let mut tx_cache = self.tx_cache.lock().unwrap();
        for tx in txs {
            let tx = tx.into();
            let txid = tx.compute_txid();
            tx_cache.insert(txid, tx);
        }
    }
}

#[allow(dead_code)]
fn insert_anchor_or_seen_at_from_status(
    update: &mut TxUpdate<ConfirmationBlockTime>,
    start_time: u64,
    txid: Txid,
    status: TxStatus,
) {
    if let TxStatus {
        block_height: Some(height),
        block_hash: Some(hash),
        block_time: Some(time),
        ..
    } = status
    {
        let anchor = ConfirmationBlockTime {
            block_id: BlockId { height, hash },
            confirmation_time: time,
        };
        update.anchors.insert((anchor, txid));
    } else {
        update.seen_ats.insert((txid, start_time));
    }
}

/// Inserts floating txouts into `tx_graph` using [`Vin`](esplora_client::api::Vin)s returned by
/// Esplora.
#[allow(dead_code)]
fn insert_prevouts(
    update: &mut TxUpdate<ConfirmationBlockTime>,
    esplora_inputs: impl IntoIterator<Item = esplora_client::api::Vin>,
) {
    let prevouts = esplora_inputs
        .into_iter()
        .filter_map(|vin| Some((vin.txid, vin.vout, vin.prevout?)));
    for (prev_txid, prev_vout, prev_txout) in prevouts {
        update.txouts.insert(
            OutPoint::new(prev_txid, prev_vout),
            TxOut {
                script_pubkey: prev_txout.scriptpubkey,
                value: Amount::from_sat(prev_txout.value),
            },
        );
    }
}
