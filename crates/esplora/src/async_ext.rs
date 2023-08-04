use async_trait::async_trait;
use bdk_chain::collections::btree_map;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, ScriptBuf, Txid},
    collections::{BTreeMap, BTreeSet},
    local_chain::{self, CheckPoint},
    BlockId, ConfirmationTimeAnchor, TxGraph,
};
use esplora_client::{Error, TxStatus};
use futures::{stream::FuturesOrdered, TryStreamExt};

use crate::{anchor_from_status, ASSUME_FINAL_DEPTH};

/// Trait to extend the functionality of [`esplora_client::AsyncClient`].
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EsploraAsyncExt {
    /// Prepare an [`LocalChain`] update with blocks fetched from Esplora.
    ///
    /// * `prev_tip` is the previous tip of [`LocalChain::tip`].
    /// * `get_heights` is the block heights that we are interested in fetching from Esplora.
    ///
    /// The result of this method can be applied to [`LocalChain::apply_update`].
    ///
    /// [`LocalChain`]: bdk_chain::local_chain::LocalChain
    /// [`LocalChain::tip`]: bdk_chain::local_chain::LocalChain::tip
    /// [`LocalChain::apply_update`]: bdk_chain::local_chain::LocalChain::apply_update
    #[allow(clippy::result_large_err)]
    async fn update_local_chain(
        &self,
        local_tip: Option<CheckPoint>,
        request_heights: impl IntoIterator<IntoIter = impl Iterator<Item = u32> + Send> + Send,
    ) -> Result<local_chain::Update, Error>;

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
    async fn update_tx_graph<K: Ord + Clone + Send>(
        &self,
        keychain_spks: BTreeMap<
            K,
            impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send,
        >,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<(TxGraph<ConfirmationTimeAnchor>, BTreeMap<K, u32>), Error>;

    /// Convenience method to call [`update_tx_graph`] without requiring a keychain.
    ///
    /// [`update_tx_graph`]: EsploraAsyncExt::update_tx_graph
    #[allow(clippy::result_large_err)]
    async fn update_tx_graph_without_keychain(
        &self,
        misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = ScriptBuf> + Send> + Send,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
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
        .await
        .map(|(g, _)| g)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl EsploraAsyncExt for esplora_client::AsyncClient {
    async fn update_local_chain(
        &self,
        local_tip: Option<CheckPoint>,
        request_heights: impl IntoIterator<IntoIter = impl Iterator<Item = u32> + Send> + Send,
    ) -> Result<local_chain::Update, Error> {
        let request_heights = request_heights.into_iter().collect::<BTreeSet<_>>();
        let new_tip_height = self.get_height().await?;

        // atomically fetch blocks from esplora
        let mut fetched_blocks = {
            let heights = (0..=new_tip_height).rev();
            let hashes = self
                .get_blocks(Some(new_tip_height))
                .await?
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
                let hash = self.get_block_hash(height).await?;
                entry.insert(hash);
            }
        }

        // find the earliest point of agreement between local chain and fetched chain
        let earliest_agreement_cp = {
            let mut earliest_agreement_cp = Option::<CheckPoint>::None;

            if let Some(local_tip) = local_tip {
                let local_tip_height = local_tip.height();
                for local_cp in local_tip.iter() {
                    let local_block = local_cp.block_id();

                    // the updated hash (block hash at this height after the update), can either be:
                    // 1. a block that already existed in `fetched_blocks`
                    // 2. a block that exists locally and atleast has a depth of ASSUME_FINAL_DEPTH
                    // 3. otherwise we can freshly fetch the block from remote, which is safe as it
                    //    is guaranteed that this would be at or below ASSUME_FINAL_DEPTH from the
                    //    remote tip
                    let updated_hash = match fetched_blocks.entry(local_block.height) {
                        btree_map::Entry::Occupied(entry) => *entry.get(),
                        btree_map::Entry::Vacant(entry) => *entry.insert(
                            if local_tip_height - local_block.height >= ASSUME_FINAL_DEPTH {
                                local_block.hash
                            } else {
                                self.get_block_hash(local_block.height).await?
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
                            .expect("must have atleast one new block");
                        if first_new_height >= local_block.height {
                            break;
                        }
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
                        .expect("must have atleast one new block");
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

    async fn update_tx_graph<K: Ord + Clone + Send>(
        &self,
        keychain_spks: BTreeMap<
            K,
            impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send,
        >,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
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
                        let client = self.clone();
                        async move {
                            let mut last_seen = None;
                            let mut spk_txs = Vec::new();
                            loop {
                                let txs = client.scripthash_txs(&spk, last_seen).await?;
                                let tx_count = txs.len();
                                last_seen = txs.last().map(|tx| tx.txid);
                                spk_txs.extend(txs);
                                if tx_count < 25 {
                                    break Result::<_, Error>::Ok((spk_index, spk_txs));
                                }
                            }
                        }
                    })
                    .collect::<FuturesOrdered<_>>();

                if handles.is_empty() {
                    break;
                }

                for (index, txs) in handles.try_collect::<Vec<TxsOfSpkIndex>>().await? {
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
                    let client = self.clone();
                    async move { client.get_tx_status(&txid).await.map(|s| (txid, s)) }
                })
                .collect::<FuturesOrdered<_>>();

            if handles.is_empty() {
                break;
            }

            for (txid, status) in handles.try_collect::<Vec<(Txid, TxStatus)>>().await? {
                if let Some(anchor) = anchor_from_status(&status) {
                    let _ = graph.insert_anchor(txid, anchor);
                }
            }
        }

        for op in outpoints.into_iter() {
            if graph.get_tx(op.txid).is_none() {
                if let Some(tx) = self.get_tx(&op.txid).await? {
                    let _ = graph.insert_tx(tx);
                }
                let status = self.get_tx_status(&op.txid).await?;
                if let Some(anchor) = anchor_from_status(&status) {
                    let _ = graph.insert_anchor(op.txid, anchor);
                }
            }

            if let Some(op_status) = self.get_output_status(&op.txid, op.vout as _).await? {
                if let Some(txid) = op_status.txid {
                    if graph.get_tx(txid).is_none() {
                        if let Some(tx) = self.get_tx(&txid).await? {
                            let _ = graph.insert_tx(tx);
                        }
                        let status = self.get_tx_status(&txid).await?;
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
