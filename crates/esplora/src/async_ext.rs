use std::collections::BTreeSet;

use async_trait::async_trait;
use bdk_chain::collections::btree_map;
use bdk_chain::Anchor;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, ScriptBuf, TxOut, Txid},
    collections::BTreeMap,
    local_chain::{self, CheckPoint},
    BlockId, ConfirmationTimeHeightAnchor, TxGraph,
};
use esplora_client::{Amount, TxStatus};
use futures::{stream::FuturesOrdered, TryStreamExt};

use crate::{anchor_from_status, FullScanUpdate, SyncUpdate};

/// [`esplora_client::Error`]
type Error = Box<esplora_client::Error>;

/// Trait to extend the functionality of [`esplora_client::AsyncClient`].
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EsploraAsyncExt {
    /// Scan keychain scripts for transactions against Esplora, returning an update that can be
    /// applied to the receiving structures.
    ///
    /// * `local_tip`: the previously seen tip from [`LocalChain::tip`].
    /// * `keychain_spks`: keychains that we want to scan transactions for
    ///
    /// The full scan for each keychain stops after a gap of `stop_gap` script pubkeys with no
    /// associated transactions. `parallel_requests` specifies the max number of HTTP requests to
    /// make in parallel.
    ///
    /// ## Note
    ///
    /// `stop_gap` is defined as "the maximum number of consecutive unused addresses".
    /// For example, with a `stop_gap` of  3, `full_scan` will keep scanning
    /// until it encounters 3 consecutive script pubkeys with no associated transactions.
    ///
    /// This follows the same approach as other Bitcoin-related software,
    /// such as [Electrum](https://electrum.readthedocs.io/en/latest/faq.html#what-is-the-gap-limit),
    /// [BTCPay Server](https://docs.btcpayserver.org/FAQ/Wallet/#the-gap-limit-problem),
    /// and [Sparrow](https://www.sparrowwallet.com/docs/faq.html#ive-restored-my-wallet-but-some-of-my-funds-are-missing).
    ///
    /// A `stop_gap` of 0 will be treated as a `stop_gap` of 1.
    ///
    /// [`LocalChain::tip`]: local_chain::LocalChain::tip
    async fn full_scan<K: Ord + Clone + Send>(
        &self,
        local_tip: CheckPoint,
        keychain_spks: BTreeMap<
            K,
            impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send,
        >,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanUpdate<K>, Error>;

    /// Sync a set of scripts with the blockchain (via an Esplora client) for the data
    /// specified and return a [`TxGraph`].
    ///
    /// * `local_tip`: the previously seen tip from [`LocalChain::tip`].
    /// * `misc_spks`: scripts that we want to sync transactions for
    /// * `txids`: transactions for which we want updated [`ConfirmationTimeHeightAnchor`]s
    /// * `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to include in the update
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`LocalChain::tip`]: local_chain::LocalChain::tip
    /// [`full_scan`]: EsploraAsyncExt::full_scan
    async fn sync(
        &self,
        local_tip: CheckPoint,
        misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = ScriptBuf> + Send> + Send,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        parallel_requests: usize,
    ) -> Result<SyncUpdate, Error>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl EsploraAsyncExt for esplora_client::AsyncClient {
    async fn full_scan<K: Ord + Clone + Send>(
        &self,
        local_tip: CheckPoint,
        keychain_spks: BTreeMap<
            K,
            impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send,
        >,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanUpdate<K>, Error> {
        let update_blocks = init_chain_update(self, &local_tip).await?;
        let (tx_graph, last_active_indices) =
            full_scan_for_index_and_graph(self, keychain_spks, stop_gap, parallel_requests).await?;
        let local_chain =
            finalize_chain_update(self, &local_tip, tx_graph.all_anchors(), update_blocks).await?;
        Ok(FullScanUpdate {
            local_chain,
            tx_graph,
            last_active_indices,
        })
    }

    async fn sync(
        &self,
        local_tip: CheckPoint,
        misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = ScriptBuf> + Send> + Send,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        parallel_requests: usize,
    ) -> Result<SyncUpdate, Error> {
        let update_blocks = init_chain_update(self, &local_tip).await?;
        let tx_graph =
            sync_for_index_and_graph(self, misc_spks, txids, outpoints, parallel_requests).await?;
        let local_chain =
            finalize_chain_update(self, &local_tip, tx_graph.all_anchors(), update_blocks).await?;
        Ok(SyncUpdate {
            tx_graph,
            local_chain,
        })
    }
}

/// Create the initial chain update.
///
/// This atomically fetches the latest blocks from Esplora and additional blocks to ensure the
/// update can connect to the `start_tip`.
///
/// We want to do this before fetching transactions and anchors as we cannot fetch latest blocks and
/// transactions atomically, and the checkpoint tip is used to determine last-scanned block (for
/// block-based chain-sources). Therefore it's better to be conservative when setting the tip (use
/// an earlier tip rather than a later tip) otherwise the caller may accidentally skip blocks when
/// alternating between chain-sources.
async fn init_chain_update(
    client: &esplora_client::AsyncClient,
    local_tip: &CheckPoint,
) -> Result<BTreeMap<u32, BlockHash>, Error> {
    // Fetch latest N (server dependent) blocks from Esplora. The server guarantees these are
    // consistent.
    let mut fetched_blocks = client
        .get_blocks(None)
        .await?
        .into_iter()
        .map(|b| (b.time.height, b.id))
        .collect::<BTreeMap<u32, BlockHash>>();
    let new_tip_height = fetched_blocks
        .keys()
        .last()
        .copied()
        .expect("must atleast have one block");

    // Ensure `fetched_blocks` can create an update that connects with the original chain by
    // finding a "Point of Agreement".
    for (height, local_hash) in local_tip.iter().map(|cp| (cp.height(), cp.hash())) {
        if height > new_tip_height {
            continue;
        }

        let fetched_hash = match fetched_blocks.entry(height) {
            btree_map::Entry::Occupied(entry) => *entry.get(),
            btree_map::Entry::Vacant(entry) => *entry.insert(client.get_block_hash(height).await?),
        };

        // We have found point of agreement so the update will connect!
        if fetched_hash == local_hash {
            break;
        }
    }

    Ok(fetched_blocks)
}

/// Fetches missing checkpoints and finalizes the [`local_chain::Update`].
///
/// A checkpoint is considered "missing" if an anchor (of `anchors`) points to a height without an
/// existing checkpoint/block under `local_tip` or `update_blocks`.
async fn finalize_chain_update<A: Anchor>(
    client: &esplora_client::AsyncClient,
    local_tip: &CheckPoint,
    anchors: &BTreeSet<(A, Txid)>,
    mut update_blocks: BTreeMap<u32, BlockHash>,
) -> Result<local_chain::Update, Error> {
    let update_tip_height = update_blocks
        .keys()
        .last()
        .copied()
        .expect("must atleast have one block");

    // We want to have a corresponding checkpoint per height. We iterate the heights of anchors
    // backwards, comparing it against our `local_tip`'s chain and our current set of
    // `update_blocks` to see if a corresponding checkpoint already exists.
    let anchor_heights = anchors
        .iter()
        .rev()
        .map(|(a, _)| a.anchor_block().height)
        // filter out heights that surpass the update tip
        .filter(|h| *h <= update_tip_height)
        // filter out duplicate heights
        .filter({
            let mut prev_height = Option::<u32>::None;
            move |h| match prev_height.replace(*h) {
                None => true,
                Some(prev_h) => prev_h != *h,
            }
        });

    // We keep track of a checkpoint node of `local_tip` to make traversing the linked-list of
    // checkpoints more efficient.
    let mut curr_cp = local_tip.clone();

    for h in anchor_heights {
        if let Some(cp) = curr_cp.range(h..).last() {
            curr_cp = cp.clone();
            if cp.height() == h {
                continue;
            }
        }
        if let btree_map::Entry::Vacant(entry) = update_blocks.entry(h) {
            entry.insert(client.get_block_hash(h).await?);
        }
    }

    Ok(local_chain::Update {
        tip: CheckPoint::from_block_ids(
            update_blocks
                .into_iter()
                .map(|(height, hash)| BlockId { height, hash }),
        )
        .expect("must be in order"),
        introduce_older_blocks: true,
    })
}

/// This performs a full scan to get an update for the [`TxGraph`] and
/// [`KeychainTxOutIndex`](bdk_chain::keychain::KeychainTxOutIndex).
async fn full_scan_for_index_and_graph<K: Ord + Clone + Send>(
    client: &esplora_client::AsyncClient,
    keychain_spks: BTreeMap<
        K,
        impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send,
    >,
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
                    let client = client.clone();
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

                    let previous_outputs = tx.vin.iter().filter_map(|vin| {
                        let prevout = vin.prevout.as_ref()?;
                        Some((
                            OutPoint {
                                txid: vin.txid,
                                vout: vin.vout,
                            },
                            TxOut {
                                script_pubkey: prevout.scriptpubkey.clone(),
                                value: Amount::from_sat(prevout.value),
                            },
                        ))
                    });

                    for (outpoint, txout) in previous_outputs {
                        let _ = graph.insert_txout(outpoint, txout);
                    }
                }
            }

            let last_index = last_index.expect("Must be set since handles wasn't empty.");
            let gap_limit_reached = if let Some(i) = last_active_index {
                last_index >= i.saturating_add(stop_gap as u32)
            } else {
                last_index + 1 >= stop_gap as u32
            };
            if gap_limit_reached {
                break;
            }
        }

        if let Some(last_active_index) = last_active_index {
            last_active_indexes.insert(keychain, last_active_index);
        }
    }

    Ok((graph, last_active_indexes))
}

async fn sync_for_index_and_graph(
    client: &esplora_client::AsyncClient,
    misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = ScriptBuf> + Send> + Send,
    txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
    outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
    parallel_requests: usize,
) -> Result<TxGraph<ConfirmationTimeHeightAnchor>, Error> {
    let mut graph = full_scan_for_index_and_graph(
        client,
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
    .await
    .map(|(g, _)| g)?;

    let mut txids = txids.into_iter();
    loop {
        let handles = txids
            .by_ref()
            .take(parallel_requests)
            .filter(|&txid| graph.get_tx(txid).is_none())
            .map(|txid| {
                let client = client.clone();
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
            if let Some(tx) = client.get_tx(&op.txid).await? {
                let _ = graph.insert_tx(tx);
            }
            let status = client.get_tx_status(&op.txid).await?;
            if let Some(anchor) = anchor_from_status(&status) {
                let _ = graph.insert_anchor(op.txid, anchor);
            }
        }

        if let Some(op_status) = client.get_output_status(&op.txid, op.vout as _).await? {
            if let Some(txid) = op_status.txid {
                if graph.get_tx(txid).is_none() {
                    if let Some(tx) = client.get_tx(&txid).await? {
                        let _ = graph.insert_tx(tx);
                    }
                    let status = client.get_tx_status(&txid).await?;
                    if let Some(anchor) = anchor_from_status(&status) {
                        let _ = graph.insert_anchor(txid, anchor);
                    }
                }
            }
        }
    }

    Ok(graph)
}

#[cfg(test)]
mod test {
    use std::{collections::BTreeSet, time::Duration};

    use bdk_chain::{
        bitcoin::{hashes::Hash, Txid},
        local_chain::LocalChain,
        BlockId,
    };
    use bdk_testenv::TestEnv;
    use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
    use esplora_client::Builder;

    use crate::async_ext::{finalize_chain_update, init_chain_update};

    macro_rules! h {
        ($index:literal) => {{
            bdk_chain::bitcoin::hashes::Hash::hash($index.as_bytes())
        }};
    }

    /// Ensure that update does not remove heights (from original), and all anchor heights are included.
    #[tokio::test]
    pub async fn test_finalize_chain_update() -> anyhow::Result<()> {
        struct TestCase<'a> {
            name: &'a str,
            /// Initial blockchain height to start the env with.
            initial_env_height: u32,
            /// Initial checkpoint heights to start with.
            initial_cps: &'a [u32],
            /// The final blockchain height of the env.
            final_env_height: u32,
            /// The anchors to test with: `(height, txid)`. Only the height is provided as we can fetch
            /// the blockhash from the env.
            anchors: &'a [(u32, Txid)],
        }

        let test_cases = [
            TestCase {
                name: "chain_extends",
                initial_env_height: 60,
                initial_cps: &[59, 60],
                final_env_height: 90,
                anchors: &[],
            },
            TestCase {
                name: "introduce_older_heights",
                initial_env_height: 50,
                initial_cps: &[10, 15],
                final_env_height: 50,
                anchors: &[(11, h!("A")), (14, h!("B"))],
            },
            TestCase {
                name: "introduce_older_heights_after_chain_extends",
                initial_env_height: 50,
                initial_cps: &[10, 15],
                final_env_height: 100,
                anchors: &[(11, h!("A")), (14, h!("B"))],
            },
        ];

        for (i, t) in test_cases.into_iter().enumerate() {
            println!("[{}] running test case: {}", i, t.name);

            let env = TestEnv::new()?;
            let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
            let client = Builder::new(base_url.as_str()).build_async()?;

            // set env to `initial_env_height`
            if let Some(to_mine) = t
                .initial_env_height
                .checked_sub(env.make_checkpoint_tip().height())
            {
                env.mine_blocks(to_mine as _, None)?;
            }
            while client.get_height().await? < t.initial_env_height {
                std::thread::sleep(Duration::from_millis(10));
            }

            // craft initial `local_chain`
            let local_chain = {
                let (mut chain, _) = LocalChain::from_genesis_hash(env.genesis_hash()?);
                let chain_tip = chain.tip();
                let update_blocks = init_chain_update(&client, &chain_tip).await?;
                let update_anchors = t
                    .initial_cps
                    .iter()
                    .map(|&height| -> anyhow::Result<_> {
                        Ok((
                            BlockId {
                                height,
                                hash: env.bitcoind.client.get_block_hash(height as _)?,
                            },
                            Txid::all_zeros(),
                        ))
                    })
                    .collect::<anyhow::Result<BTreeSet<_>>>()?;
                let chain_update =
                    finalize_chain_update(&client, &chain_tip, &update_anchors, update_blocks)
                        .await?;
                chain.apply_update(chain_update)?;
                chain
            };
            println!("local chain height: {}", local_chain.tip().height());

            // extend env chain
            if let Some(to_mine) = t
                .final_env_height
                .checked_sub(env.make_checkpoint_tip().height())
            {
                env.mine_blocks(to_mine as _, None)?;
            }
            while client.get_height().await? < t.final_env_height {
                std::thread::sleep(Duration::from_millis(10));
            }

            // craft update
            let update = {
                let local_tip = local_chain.tip();
                let update_blocks = init_chain_update(&client, &local_tip).await?;
                let update_anchors = t
                    .anchors
                    .iter()
                    .map(|&(height, txid)| -> anyhow::Result<_> {
                        Ok((
                            BlockId {
                                height,
                                hash: env.bitcoind.client.get_block_hash(height as _)?,
                            },
                            txid,
                        ))
                    })
                    .collect::<anyhow::Result<_>>()?;
                finalize_chain_update(&client, &local_tip, &update_anchors, update_blocks).await?
            };

            // apply update
            let mut updated_local_chain = local_chain.clone();
            updated_local_chain.apply_update(update)?;
            println!(
                "updated local chain height: {}",
                updated_local_chain.tip().height()
            );

            assert!(
                {
                    let initial_heights = local_chain
                        .iter_checkpoints()
                        .map(|cp| cp.height())
                        .collect::<BTreeSet<_>>();
                    let updated_heights = updated_local_chain
                        .iter_checkpoints()
                        .map(|cp| cp.height())
                        .collect::<BTreeSet<_>>();
                    updated_heights.is_superset(&initial_heights)
                },
                "heights from the initial chain must all be in the updated chain",
            );

            assert!(
                {
                    let exp_anchor_heights = t
                        .anchors
                        .iter()
                        .map(|(h, _)| *h)
                        .chain(t.initial_cps.iter().copied())
                        .collect::<BTreeSet<_>>();
                    let anchor_heights = updated_local_chain
                        .iter_checkpoints()
                        .map(|cp| cp.height())
                        .collect::<BTreeSet<_>>();
                    anchor_heights.is_superset(&exp_anchor_heights)
                },
                "anchor heights must all be in updated chain",
            );
        }

        Ok(())
    }
}
