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

use bdk_core::bitcoin::{Amount, OutPoint, TxOut, Txid};
use bdk_core::{BlockId, ConfirmationBlockTime, TxUpdate};
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

/// Update the stop-gap state with the result for script `index`.
///
/// Returns `false` (leaving the state untouched) if `gap_limit` consecutive unused scripts were
/// already seen, meaning the result must not be processed.
#[allow(dead_code)]
fn track_spk_activity(
    has_txs: bool,
    index: u32,
    last_revealed: Option<u32>,
    gap_limit: usize,
    consecutive_unused: &mut usize,
    last_active_index: &mut Option<u32>,
) -> bool {
    if *consecutive_unused >= gap_limit {
        return false;
    }
    if has_txs {
        *consecutive_unused = 0;
        *last_active_index = Some(index);
    } else if last_revealed.is_none_or(|lr| index > lr) {
        *consecutive_unused = consecutive_unused.saturating_add(1);
    }
    true
}

#[cfg(test)]
mod gap_tests {
    use super::track_spk_activity;

    /// Run results (index, has_txs) through the tracker like a full scan loop does.
    fn run(results: &[(u32, bool)], gap: usize, revealed: Option<u32>) -> (Vec<u32>, Option<u32>) {
        let (mut unused, mut last_active) = (0usize, None);
        let mut processed = vec![];
        for &(i, has_txs) in results {
            if !track_spk_activity(has_txs, i, revealed, gap, &mut unused, &mut last_active) {
                break;
            }
            processed.push(i);
        }
        (processed, last_active)
    }

    #[test]
    fn ignores_results_past_stop_gap_in_same_batch() {
        // stop_gap = 2: index 2 (active) is in the same batch but past the gap.
        let (processed, last_active) = run(&[(0, false), (1, false), (2, true)], 2, None);
        assert_eq!(processed, vec![0, 1]);
        assert_eq!(last_active, None);
    }

    #[test]
    fn same_result_regardless_of_batching() {
        let r = [(0, true), (1, false), (2, false), (3, true), (4, false)];
        assert_eq!(run(&r, 2, None), (vec![0, 1, 2], Some(0)));
    }

    #[test]
    fn activity_within_gap_resets_counter() {
        let r = [(0, false), (1, true), (2, false), (3, false), (4, true)];
        assert_eq!(run(&r, 2, None), (vec![0, 1, 2, 3], Some(1)));
    }

    #[test]
    fn revealed_indices_do_not_count_as_unused() {
        let r = [(0, false), (1, false), (2, false), (3, true)];
        assert_eq!(run(&r, 2, Some(2)), (vec![0, 1, 2, 3], Some(3)));
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
