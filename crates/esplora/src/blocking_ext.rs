use std::thread::JoinHandle;

use bdk_chain::collections::btree_map;
use bdk_chain::collections::{BTreeMap, BTreeSet};
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, ScriptBuf, Txid},
    local_chain::{self, CheckPoint},
    BlockId, ConfirmationTimeHeightAnchor, TxGraph,
};
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
    /// * `local_tip` is the previous tip of [`LocalChain::tip`].
    /// * `request_heights` is the block heights that we are interested in fetching from Esplora.
    ///
    /// The result of this method can be applied to [`LocalChain::apply_update`].
    ///
    /// [`LocalChain`]: bdk_chain::local_chain::LocalChain
    /// [`LocalChain::tip`]: bdk_chain::local_chain::LocalChain::tip
    /// [`LocalChain::apply_update`]: bdk_chain::local_chain::LocalChain::apply_update
    #[allow(clippy::result_large_err)]
    fn update_local_chain(
        &self,
        local_tip: CheckPoint,
        request_heights: impl IntoIterator<Item = u32>,
    ) -> Result<local_chain::Update, Error>;

    /// Full scan the keychain scripts specified with the blockchain (via an Esplora client) and
    /// returns a [`TxGraph`] and a map of last active indices.
    ///
    /// * `keychain_spks`: keychains that we want to scan transactions for
    ///
    /// The full scan for each keychain stops after a gap of `stop_gap` script pubkeys with no associated
    /// transactions. `parallel_requests` specifies the max number of HTTP requests to make in
    /// parallel.
    #[allow(clippy::result_large_err)]
    fn full_scan<K: Ord + Clone>(
        &self,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, ScriptBuf)>>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<(TxGraph<ConfirmationTimeHeightAnchor>, BTreeMap<K, u32>), Error>;

    /// Sync a set of scripts with the blockchain (via an Esplora client) for the data
    /// specified and return a [`TxGraph`].
    ///
    /// * `misc_spks`: scripts that we want to sync transactions for
    /// * `txids`: transactions for which we want updated [`ConfirmationTimeHeightAnchor`]s
    /// * `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to include in the update
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`full_scan`]: EsploraExt::full_scan
    #[allow(clippy::result_large_err)]
    fn sync(
        &self,
        misc_spks: impl IntoIterator<Item = ScriptBuf>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        parallel_requests: usize,
    ) -> Result<TxGraph<ConfirmationTimeHeightAnchor>, Error>;
}

impl EsploraExt for esplora_client::BlockingClient {
    fn update_local_chain(
        &self,
        local_tip: CheckPoint,
        request_heights: impl IntoIterator<Item = u32>,
    ) -> Result<local_chain::Update, Error> {
        let request_heights = request_heights.into_iter().collect::<BTreeSet<_>>();
        let new_tip_height = self.get_height()?;

        // atomically fetch blocks from esplora
        let mut fetched_blocks = {
            let heights = (0..=new_tip_height).rev();
            let hashes = self
                .get_blocks(Some(new_tip_height))?
                .into_iter()
                .map(|b| b.id);
            heights.zip(hashes).collect::<BTreeMap<u32, BlockHash>>()
        };

        // fetch heights that the caller is interested in
        for height in request_heights {
            // do not fetch blocks higher than remote tip
            if height > new_tip_height {
                continue;
            }
            // only fetch what is missing
            if let btree_map::Entry::Vacant(entry) = fetched_blocks.entry(height) {
                let hash = self.get_block_hash(height)?;
                entry.insert(hash);
            }
        }

        // find the earliest point of agreement between local chain and fetched chain
        let earliest_agreement_cp = {
            let mut earliest_agreement_cp = Option::<CheckPoint>::None;

            let local_tip_height = local_tip.height();
            for local_cp in local_tip.iter() {
                let local_block = local_cp.block_id();

                // the updated hash (block hash at this height after the update), can either be:
                // 1. a block that already existed in `fetched_blocks`
                // 2. a block that exists locally and at least has a depth of ASSUME_FINAL_DEPTH
                // 3. otherwise we can freshly fetch the block from remote, which is safe as it
                //    is guaranteed that this would be at or below ASSUME_FINAL_DEPTH from the
                //    remote tip
                let updated_hash = match fetched_blocks.entry(local_block.height) {
                    btree_map::Entry::Occupied(entry) => *entry.get(),
                    btree_map::Entry::Vacant(entry) => *entry.insert(
                        if local_tip_height - local_block.height >= ASSUME_FINAL_DEPTH {
                            local_block.hash
                        } else {
                            self.get_block_hash(local_block.height)?
                        },
                    ),
                };

                // since we may introduce blocks below the point of agreement, we cannot break
                // here unconditionally - we only break if we guarantee there are no new heights
                // below our current local checkpoint
                if local_block.hash == updated_hash {
                    earliest_agreement_cp = Some(local_cp);

                    let first_new_height = *fetched_blocks
                        .keys()
                        .next()
                        .expect("must have at least one new block");
                    if first_new_height >= local_block.height {
                        break;
                    }
                }
            }

            earliest_agreement_cp
        };

        let tip = {
            // first checkpoint to use for the update chain
            let first_cp = match earliest_agreement_cp {
                Some(cp) => cp,
                None => {
                    let (&height, &hash) = fetched_blocks
                        .iter()
                        .next()
                        .expect("must have at least one new block");
                    CheckPoint::new(BlockId { height, hash })
                }
            };
            // transform fetched chain into the update chain
            fetched_blocks
                // we exclude anything at or below the first cp of the update chain otherwise
                // building the chain will fail
                .split_off(&(first_cp.height() + 1))
                .into_iter()
                .map(|(height, hash)| BlockId { height, hash })
                .fold(first_cp, |prev_cp, block| {
                    prev_cp.push(block).expect("must extend checkpoint")
                })
        };

        Ok(local_chain::Update {
            tip,
            introduce_older_blocks: true,
        })
    }

    fn full_scan<K: Ord + Clone>(
        &self,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, ScriptBuf)>>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<(TxGraph<ConfirmationTimeHeightAnchor>, BTreeMap<K, u32>), Error> {
        type TxsOfSpkIndex = (u32, Vec<esplora_client::Tx>);
        let parallel_requests = Ord::max(parallel_requests, 1);
        let mut graph = TxGraph::<ConfirmationTimeHeightAnchor>::default();
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

                let last_index = last_index.expect("Must be set since handles wasn't empty.");
                let past_gap_limit = if let Some(i) = last_active_index {
                    last_index > i.saturating_add(stop_gap as u32)
                } else {
                    last_index >= stop_gap as u32
                };
                if past_gap_limit {
                    break;
                }
            }

            if let Some(last_active_index) = last_active_index {
                last_active_indexes.insert(keychain, last_active_index);
            }
        }

        Ok((graph, last_active_indexes))
    }

    fn sync(
        &self,
        misc_spks: impl IntoIterator<Item = ScriptBuf>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        parallel_requests: usize,
    ) -> Result<TxGraph<ConfirmationTimeHeightAnchor>, Error> {
        let mut graph = self
            .full_scan(
                [(
                    (),
                    misc_spks
                        .into_iter()
                        .enumerate()
                        .map(|(i, spk)| (i as u32, spk)),
                )]
                .into(),
                usize::MAX,
                parallel_requests,
            )
            .map(|(g, _)| g)?;

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

        for op in outpoints {
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
        Ok(graph)
    }
}
