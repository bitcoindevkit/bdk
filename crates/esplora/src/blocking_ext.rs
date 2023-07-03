use std::thread::JoinHandle;

use bdk_chain::bitcoin::{OutPoint, Txid};
use bdk_chain::collections::btree_map;
use bdk_chain::collections::BTreeMap;
use bdk_chain::{
    bitcoin::{BlockHash, Script},
    local_chain::CheckPoint,
};
use bdk_chain::{BlockId, ConfirmationTimeAnchor, TxGraph};
use esplora_client::{Error, TxStatus};

use crate::{anchor_from_status, ASSUME_FINAL_DEPTH};

/// Trait to extend the functionality of [`esplora_client::BlockingClient`].
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
pub trait EsploraExt {
    /// Prepare an [`LocalChain`] update with blocks fetched from Esplora.
    ///
    /// * `prev_tip` is the previous tip of [`LocalChain::tip`].
    /// * `get_heights` is the block heights that we are interested in fetching from Esplora.
    ///
    /// The result of this method can be applied to [`LocalChain::update`].
    ///
    /// [`LocalChain`]: bdk_chain::local_chain::LocalChain
    /// [`LocalChain::tip`]: bdk_chain::local_chain::LocalChain::tip
    /// [`LocalChain::update`]: bdk_chain::local_chain::LocalChain::update
    #[allow(clippy::result_large_err)]
    fn update_local_chain(
        &self,
        prev_tip: Option<CheckPoint>,
        get_heights: impl IntoIterator<Item = u32>,
    ) -> Result<CheckPoint, Error>;

    /// Scan Esplora for the data specified and return a [`TxGraph`] and a map of last active
    /// indices.
    ///
    /// * `keychain_spks`: keychains that we want to scan transactions for
    /// * `txids`: transactions for which we want updated [`ConfirmationTimeAnchor`]s
    /// * `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to include in the update
    ///
    /// The scan for each keychain stops after a gap of `stop_gap` script pubkeys with no associated
    /// transactions. `parallel_requests` specifies the max number of HTTP requests to make in
    /// parallel.
    #[allow(clippy::result_large_err)]
    fn update_tx_graph<K: Ord + Clone>(
        &self,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<(TxGraph<ConfirmationTimeAnchor>, BTreeMap<K, u32>), Error>;

    /// Convenience method to call [`update_tx_graph`] without requiring a keychain.
    ///
    /// [`update_tx_graph`]: EsploraExt::update_tx_graph
    #[allow(clippy::result_large_err)]
    fn update_tx_graph_without_keychain(
        &self,
        misc_spks: impl IntoIterator<Item = Script>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        parallel_requests: usize,
    ) -> Result<TxGraph<ConfirmationTimeAnchor>, Error> {
        self.update_tx_graph(
            [(
                (),
                misc_spks
                    .into_iter()
                    .enumerate()
                    .map(|(i, spk)| (i as u32, spk)),
            )]
            .into(),
            txids,
            outpoints,
            usize::MAX,
            parallel_requests,
        )
        .map(|(g, _)| g)
    }
}

impl EsploraExt for esplora_client::BlockingClient {
    fn update_local_chain(
        &self,
        prev_tip: Option<CheckPoint>,
        get_heights: impl IntoIterator<Item = u32>,
    ) -> Result<CheckPoint, Error> {
        let new_tip_height = self.get_height()?;

        // If esplora returns a tip height that is lower than our previous tip, then checkpoints do
        // not need updating. We just return the previous tip and use that as the point of
        // agreement.
        if let Some(prev_tip) = prev_tip.as_ref() {
            if new_tip_height < prev_tip.height() {
                return Ok(prev_tip.clone());
            }
        }

        // Fetch new block IDs that are to be included in the update. This includes:
        // 1. Atomically fetched most-recent blocks so we have a consistent view even during reorgs.
        // 2. Heights the caller is interested in (as specified in `get_heights`).
        let mut new_blocks = {
            let heights = (0..=new_tip_height).rev();
            let hashes = self
                .get_blocks(Some(new_tip_height))?
                .into_iter()
                .map(|b| b.id);

            let mut new_blocks = heights.zip(hashes).collect::<BTreeMap<u32, BlockHash>>();

            for height in get_heights {
                // do not fetch blocks higher than known tip
                if height > new_tip_height {
                    continue;
                }
                if let btree_map::Entry::Vacant(entry) = new_blocks.entry(height) {
                    let hash = self.get_block_hash(height)?;
                    entry.insert(hash);
                }
            }

            new_blocks
        };

        // Determine the checkpoint to start building our update tip from.
        let first_cp = match prev_tip {
            Some(old_tip) => {
                let old_tip_height = old_tip.height();
                let mut earliest_agreement_cp = Option::<CheckPoint>::None;

                for old_cp in old_tip.iter() {
                    let old_block = old_cp.block_id();

                    let new_hash = match new_blocks.entry(old_block.height) {
                        btree_map::Entry::Vacant(entry) => *entry.insert(
                            if old_tip_height - old_block.height >= ASSUME_FINAL_DEPTH {
                                old_block.hash
                            } else {
                                self.get_block_hash(old_block.height)?
                            },
                        ),
                        btree_map::Entry::Occupied(entry) => *entry.get(),
                    };

                    // Since we may introduce blocks below the point of agreement, we cannot break
                    // here unconditionally. We only break if we guarantee there are no new heights
                    // below our current.
                    if old_block.hash == new_hash {
                        earliest_agreement_cp = Some(old_cp);

                        let first_new_height = *new_blocks
                            .keys()
                            .next()
                            .expect("must have atleast one new block");
                        if first_new_height <= old_block.height {
                            break;
                        }
                    }
                }

                earliest_agreement_cp
            }
            None => None,
        }
        .unwrap_or_else(|| {
            let (&height, &hash) = new_blocks
                .iter()
                .next()
                .expect("must have atleast one new block");
            CheckPoint::new(BlockId { height, hash })
        });

        let new_tip = new_blocks
            .split_off(&(first_cp.height() + 1))
            .into_iter()
            .map(|(height, hash)| BlockId { height, hash })
            .fold(first_cp, |prev_cp, block| {
                prev_cp.extend(block).expect("must extend checkpoint")
            });

        Ok(new_tip)
    }

    fn update_tx_graph<K: Ord + Clone>(
        &self,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<(TxGraph<ConfirmationTimeAnchor>, BTreeMap<K, u32>), Error> {
        type TxsOfSpkIndex = (u32, Vec<esplora_client::Tx>);
        let parallel_requests = Ord::max(parallel_requests, 1);
        let mut graph = TxGraph::<ConfirmationTimeAnchor>::default();
        let mut last_active_indexes = BTreeMap::<K, u32>::new();

        for (keychain, spks) in keychain_spks {
            let mut spks = spks.into_iter();
            let mut last_index = Option::<u32>::None;
            let mut last_active_index = Option::<u32>::None;

            loop {
                let handles = spks
                    .by_ref()
                    .take(parallel_requests)
                    .map(|(spk_index, spk)| {
                        std::thread::spawn({
                            let client = self.clone();
                            move || -> Result<TxsOfSpkIndex, Error> {
                                let mut last_seen = None;
                                let mut spk_txs = Vec::new();
                                loop {
                                    let txs = client.scripthash_txs(&spk, last_seen)?;
                                    let tx_count = txs.len();
                                    last_seen = txs.last().map(|tx| tx.txid);
                                    spk_txs.extend(txs);
                                    if tx_count < 25 {
                                        break Ok((spk_index, spk_txs));
                                    }
                                }
                            }
                        })
                    })
                    .collect::<Vec<JoinHandle<Result<TxsOfSpkIndex, Error>>>>();

                if handles.is_empty() {
                    break;
                }

                for handle in handles {
                    let (index, txs) = handle.join().expect("thread must not panic")?;
                    last_index = Some(index);
                    if !txs.is_empty() {
                        last_active_index = Some(index);
                    }
                    for tx in txs {
                        let _ = graph.insert_tx(tx.to_tx());
                        if let Some(anchor) = anchor_from_status(&tx.status) {
                            let _ = graph.insert_anchor(tx.txid, anchor);
                        }
                    }
                }

                if last_index > last_active_index.map(|i| i + stop_gap as u32) {
                    break;
                }
            }

            if let Some(last_active_index) = last_active_index {
                last_active_indexes.insert(keychain, last_active_index);
            }
        }

        let mut txids = txids.into_iter();
        loop {
            let handles = txids
                .by_ref()
                .take(parallel_requests)
                .filter(|&txid| graph.get_tx(txid).is_none())
                .map(|txid| {
                    std::thread::spawn({
                        let client = self.clone();
                        move || client.get_tx_status(&txid).map(|s| (txid, s))
                    })
                })
                .collect::<Vec<JoinHandle<Result<(Txid, TxStatus), Error>>>>();

            if handles.is_empty() {
                break;
            }

            for handle in handles {
                let (txid, status) = handle.join().expect("thread must not panic")?;
                if let Some(anchor) = anchor_from_status(&status) {
                    let _ = graph.insert_anchor(txid, anchor);
                }
            }
        }

        for op in outpoints.into_iter() {
            if graph.get_tx(op.txid).is_none() {
                if let Some(tx) = self.get_tx(&op.txid)? {
                    let _ = graph.insert_tx(tx);
                }
                let status = self.get_tx_status(&op.txid)?;
                if let Some(anchor) = anchor_from_status(&status) {
                    let _ = graph.insert_anchor(op.txid, anchor);
                }
            }

            if let Some(op_status) = self.get_output_status(&op.txid, op.vout as _)? {
                if let Some(txid) = op_status.txid {
                    if graph.get_tx(txid).is_none() {
                        if let Some(tx) = self.get_tx(&txid)? {
                            let _ = graph.insert_tx(tx);
                        }
                        let status = self.get_tx_status(&txid)?;
                        if let Some(anchor) = anchor_from_status(&status) {
                            let _ = graph.insert_anchor(txid, anchor);
                        }
                    }
                }
            }
        }

        Ok((graph, last_active_indexes))
    }
}
