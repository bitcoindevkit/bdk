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

use bdk_core::bitcoin::{Amount, OutPoint, TxOut, Txid};
use bdk_core::{tx_graph, BlockId, ConfirmationBlockTime};
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

fn insert_anchor_from_status(
    update: &mut tx_graph::Update<ConfirmationBlockTime>,
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
    }
}

/// Inserts floating txouts into `tx_graph` using [`Vin`](esplora_client::api::Vin)s returned by
/// Esplora.
fn insert_prevouts(
    update: &mut tx_graph::Update<ConfirmationBlockTime>,
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
