use async_trait::async_trait;
use bdk_core::collections::{BTreeMap, BTreeSet, HashSet};
use bdk_core::spk_client::{
    FullScanRequest, FullScanResponse, SpkWithExpectedTxids, SyncRequest, SyncResponse,
};
use bdk_core::{
    bitcoin::{BlockHash, OutPoint, Txid},
    BlockId, CheckPoint, ConfirmationBlockTime, Indexed, TxUpdate,
};
use esplora_client::Sleeper;
use futures::{stream::FuturesOrdered, TryStreamExt};

use crate::{insert_anchor_from_status, insert_prevouts};

/// [`esplora_client::Error`]
type Error = Box<esplora_client::Error>;

/// Trait to extend the functionality of [`esplora_client::AsyncClient`].
///
/// Refer to [crate-level documentation](crate) for more.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EsploraAsyncExt {
    /// Scan keychain scripts for transactions against Esplora, returning an update that can be
    /// applied to the receiving structures.
    ///
    /// `request` provides the data required to perform a script-pubkey-based full scan
    /// (see [`FullScanRequest`]). The full scan for each keychain (`K`) stops after a gap of
    /// `stop_gap` script pubkeys with no associated transactions. `parallel_requests` specifies
    /// the maximum number of HTTP requests to make in parallel.
    ///
    /// Refer to [crate-level docs](crate) for more.
    async fn full_scan<K: Ord + Clone + Send, R: Into<FullScanRequest<K>> + Send>(
        &self,
        request: R,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResponse<K>, Error>;

    /// Sync a set of scripts, txids, and/or outpoints against Esplora.
    ///
    /// `request` provides the data required to perform a script-pubkey-based sync (see
    /// [`SyncRequest`]). `parallel_requests` specifies the maximum number of HTTP requests to make
    /// in parallel.
    ///
    /// Refer to [crate-level docs](crate) for more.
    async fn sync<I: Send, R: Into<SyncRequest<I>> + Send>(
        &self,
        request: R,
        parallel_requests: usize,
    ) -> Result<SyncResponse, Error>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<S> EsploraAsyncExt for esplora_client::AsyncClient<S>
where
    S: Sleeper + Clone + Send + Sync,
    S::Sleep: Send,
{
    async fn full_scan<K: Ord + Clone + Send, R: Into<FullScanRequest<K>> + Send>(
        &self,
        request: R,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResponse<K>, Error> {
        let mut request = request.into();
        let keychains = request.keychains();

        let chain_tip = request.chain_tip();
        let latest_blocks = if chain_tip.is_some() {
            Some(fetch_latest_blocks(self).await?)
        } else {
            None
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        let mut inserted_txs = HashSet::<Txid>::new();
        let mut last_active_indices = BTreeMap::<K, u32>::new();
        for keychain in keychains {
            let keychain_spks = request.iter_spks(keychain.clone());
            let spks_with_history = keychain_spks.into_iter().map(|(i, spk)| {
                (
                    i,
                    SpkWithExpectedTxids {
                        spk,
                        txids: HashSet::<Txid>::new(),
                    },
                )
            });
            let (update, last_active_index) = fetch_txs_with_keychain_spks(
                self,
                &mut inserted_txs,
                spks_with_history,
                stop_gap,
                parallel_requests,
            )
            .await?;
            tx_update.extend(update);
            if let Some(last_active_index) = last_active_index {
                last_active_indices.insert(keychain, last_active_index);
            }
        }

        let chain_update = match (chain_tip, latest_blocks) {
            (Some(chain_tip), Some(latest_blocks)) => {
                Some(chain_update(self, &latest_blocks, &chain_tip, &tx_update.anchors).await?)
            }
            _ => None,
        };

        Ok(FullScanResponse {
            chain_update,
            tx_update,
            last_active_indices,
        })
    }

    async fn sync<I: Send, R: Into<SyncRequest<I>> + Send>(
        &self,
        request: R,
        parallel_requests: usize,
    ) -> Result<SyncResponse, Error> {
        let mut request = request.into();

        let chain_tip = request.chain_tip();
        let latest_blocks = if chain_tip.is_some() {
            Some(fetch_latest_blocks(self).await?)
        } else {
            None
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        let mut inserted_txs = HashSet::<Txid>::new();
        tx_update.extend(
            fetch_txs_with_spks(
                self,
                &mut inserted_txs,
                request.iter_spks_with_expected_txids(),
                parallel_requests,
            )
            .await?,
        );
        tx_update.extend(
            fetch_txs_with_txids(
                self,
                &mut inserted_txs,
                request.iter_txids(),
                parallel_requests,
            )
            .await?,
        );
        tx_update.extend(
            fetch_txs_with_outpoints(
                self,
                &mut inserted_txs,
                request.iter_outpoints(),
                parallel_requests,
            )
            .await?,
        );

        let chain_update = match (chain_tip, latest_blocks) {
            (Some(chain_tip), Some(latest_blocks)) => {
                Some(chain_update(self, &latest_blocks, &chain_tip, &tx_update.anchors).await?)
            }
            _ => None,
        };

        Ok(SyncResponse {
            chain_update,
            tx_update,
        })
    }
}

/// Fetch latest blocks from Esplora in an atomic call.
///
/// We want to do this before fetching transactions and anchors as we cannot fetch latest blocks AND
/// transactions atomically, and the checkpoint tip is used to determine last-scanned block (for
/// block-based chain-sources). Therefore it's better to be conservative when setting the tip (use
/// an earlier tip rather than a later tip) otherwise the caller may accidentally skip blocks when
/// alternating between chain-sources.
async fn fetch_latest_blocks<S: Sleeper>(
    client: &esplora_client::AsyncClient<S>,
) -> Result<BTreeMap<u32, BlockHash>, Error> {
    Ok(client
        .get_blocks(None)
        .await?
        .into_iter()
        .map(|b| (b.time.height, b.id))
        .collect())
}

/// Used instead of [`esplora_client::BlockingClient::get_block_hash`].
///
/// This first checks the previously fetched `latest_blocks` before fetching from Esplora again.
async fn fetch_block<S: Sleeper>(
    client: &esplora_client::AsyncClient<S>,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    height: u32,
) -> Result<Option<BlockHash>, Error> {
    if let Some(&hash) = latest_blocks.get(&height) {
        return Ok(Some(hash));
    }

    // We avoid fetching blocks higher than previously fetched `latest_blocks` as the local chain
    // tip is used to signal for the last-synced-up-to-height.
    let &tip_height = latest_blocks
        .keys()
        .last()
        .expect("must have atleast one entry");
    if height > tip_height {
        return Ok(None);
    }

    Ok(Some(client.get_block_hash(height).await?))
}

/// Create the [`local_chain::Update`].
///
/// We want to have a corresponding checkpoint per anchor height. However, checkpoints fetched
/// should not surpass `latest_blocks`.
async fn chain_update<S: Sleeper>(
    client: &esplora_client::AsyncClient<S>,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    local_tip: &CheckPoint,
    anchors: &BTreeSet<(ConfirmationBlockTime, Txid)>,
) -> Result<CheckPoint, Error> {
    let mut point_of_agreement = None;
    let mut conflicts = vec![];
    for local_cp in local_tip.iter() {
        let remote_hash = match fetch_block(client, latest_blocks, local_cp.height()).await? {
            Some(hash) => hash,
            None => continue,
        };
        if remote_hash == local_cp.hash() {
            point_of_agreement = Some(local_cp.clone());
            break;
        } else {
            // it is not strictly necessary to include all the conflicted heights (we do need the
            // first one) but it seems prudent to make sure the updated chain's heights are a
            // superset of the existing chain after update.
            conflicts.push(BlockId {
                height: local_cp.height(),
                hash: remote_hash,
            });
        }
    }

    let mut tip = point_of_agreement.expect("remote esplora should have same genesis block");

    tip = tip
        .extend(conflicts.into_iter().rev())
        .expect("evicted are in order");

    for (anchor, _txid) in anchors {
        let height = anchor.block_id.height;
        if tip.get(height).is_none() {
            let hash = match fetch_block(client, latest_blocks, height).await? {
                Some(hash) => hash,
                None => continue,
            };
            tip = tip.insert(BlockId { height, hash });
        }
    }

    // insert the most recent blocks at the tip to make sure we update the tip and make the update
    // robust.
    for (&height, &hash) in latest_blocks.iter() {
        tip = tip.insert(BlockId { height, hash });
    }

    Ok(tip)
}

/// Fetch transactions and associated [`ConfirmationBlockTime`]s by scanning
/// `keychain_spks` against Esplora.
///
/// `keychain_spks` is an *unbounded* indexed-[`ScriptBuf`] iterator that represents scripts
/// derived from a keychain. The scanning logic stops after a `stop_gap` number of consecutive
/// scripts with no transaction history is reached. `parallel_requests` specifies the maximum
/// number of HTTP requests to make in parallel.
///
/// A [`TxGraph`] (containing the fetched transactions and anchors) and the last active
/// keychain index (if any) is returned. The last active keychain index is the keychain's last
/// script pubkey that contains a non-empty transaction history.
///
/// Refer to [crate-level docs](crate) for more.
async fn fetch_txs_with_keychain_spks<I, S>(
    client: &esplora_client::AsyncClient<S>,
    inserted_txs: &mut HashSet<Txid>,
    mut spks_with_history: I,
    stop_gap: usize,
    parallel_requests: usize,
) -> Result<(TxUpdate<ConfirmationBlockTime>, Option<u32>), Error>
where
    I: Iterator<Item = Indexed<SpkWithExpectedTxids>> + Send,
    S: Sleeper + Clone + Send + Sync,
{
    type TxsOfSpkIndex = (u32, Vec<esplora_client::Tx>);

    let mut update = TxUpdate::<ConfirmationBlockTime>::default();
    let mut last_index = Option::<u32>::None;
    let mut last_active_index = Option::<u32>::None;
    let mut spk_txids = HashSet::new();

    loop {
        let handles = spks_with_history
            .by_ref()
            .take(parallel_requests)
            .map(|(spk_index, spk_with_history)| {
                spk_txids.extend(&spk_with_history.txids);
                let client = client.clone();
                async move {
                    let mut last_seen = None;
                    let mut spk_txs = Vec::new();
                    loop {
                        let txs = client
                            .scripthash_txs(&spk_with_history.spk, last_seen)
                            .await?;
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
                if inserted_txs.insert(tx.txid) {
                    update.txs.push(tx.to_tx().into());
                }
                insert_anchor_from_status(&mut update, tx.txid, tx.status);
                insert_prevouts(&mut update, tx.vin);
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

    update
        .evicted
        .extend(spk_txids.difference(inserted_txs).cloned());

    Ok((update, last_active_index))
}

/// Fetch transactions and associated [`ConfirmationBlockTime`]s by scanning `spks`
/// against Esplora.
///
/// Unlike with [`EsploraAsyncExt::fetch_txs_with_keychain_spks`], `spks` must be *bounded* as
/// all contained scripts will be scanned. `parallel_requests` specifies the maximum number of
/// HTTP requests to make in parallel.
///
/// Refer to [crate-level docs](crate) for more.
async fn fetch_txs_with_spks<I, S>(
    client: &esplora_client::AsyncClient<S>,
    inserted_txs: &mut HashSet<Txid>,
    spks_with_history: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error>
where
    I: IntoIterator<Item = SpkWithExpectedTxids> + Send,
    I::IntoIter: Send,
    S: Sleeper + Clone + Send + Sync,
{
    fetch_txs_with_keychain_spks(
        client,
        inserted_txs,
        spks_with_history
            .into_iter()
            .enumerate()
            .map(|(i, spk)| (i as u32, spk)),
        usize::MAX,
        parallel_requests,
    )
    .await
    .map(|(update, _)| update)
}

/// Fetch transactions and associated [`ConfirmationBlockTime`]s by scanning `txids`
/// against Esplora.
///
/// `parallel_requests` specifies the maximum number of HTTP requests to make in parallel.
///
/// Refer to [crate-level docs](crate) for more.
async fn fetch_txs_with_txids<I, S>(
    client: &esplora_client::AsyncClient<S>,
    inserted_txs: &mut HashSet<Txid>,
    txids: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error>
where
    I: IntoIterator<Item = Txid> + Send,
    I::IntoIter: Send,
    S: Sleeper + Clone + Send + Sync,
{
    let mut update = TxUpdate::<ConfirmationBlockTime>::default();
    // Only fetch for non-inserted txs.
    let mut txids = txids
        .into_iter()
        .filter(|txid| !inserted_txs.contains(txid))
        .collect::<Vec<Txid>>()
        .into_iter();
    loop {
        let handles = txids
            .by_ref()
            .take(parallel_requests)
            .map(|txid| {
                let client = client.clone();
                async move { client.get_tx_info(&txid).await.map(|t| (txid, t)) }
            })
            .collect::<FuturesOrdered<_>>();

        if handles.is_empty() {
            break;
        }

        for (txid, tx_info) in handles.try_collect::<Vec<_>>().await? {
            if let Some(tx_info) = tx_info {
                if inserted_txs.insert(txid) {
                    update.txs.push(tx_info.to_tx().into());
                }
                insert_anchor_from_status(&mut update, txid, tx_info.status);
                insert_prevouts(&mut update, tx_info.vin);
            }
        }
    }
    Ok(update)
}

/// Fetch transactions and [`ConfirmationBlockTime`]s that contain and spend the provided
/// `outpoints`.
///
/// `parallel_requests` specifies the maximum number of HTTP requests to make in parallel.
///
/// Refer to [crate-level docs](crate) for more.
async fn fetch_txs_with_outpoints<I, S>(
    client: &esplora_client::AsyncClient<S>,
    inserted_txs: &mut HashSet<Txid>,
    outpoints: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error>
where
    I: IntoIterator<Item = OutPoint> + Send,
    I::IntoIter: Send,
    S: Sleeper + Clone + Send + Sync,
{
    let outpoints = outpoints.into_iter().collect::<Vec<_>>();
    let mut update = TxUpdate::<ConfirmationBlockTime>::default();

    // make sure txs exists in graph and tx statuses are updated
    // TODO: We should maintain a tx cache (like we do with Electrum).
    update.extend(
        fetch_txs_with_txids(
            client,
            inserted_txs,
            outpoints.iter().copied().map(|op| op.txid),
            parallel_requests,
        )
        .await?,
    );

    // get outpoint spend-statuses
    let mut outpoints = outpoints.into_iter();
    let mut missing_txs = Vec::<Txid>::with_capacity(outpoints.len());
    loop {
        let handles = outpoints
            .by_ref()
            .take(parallel_requests)
            .map(|op| {
                let client = client.clone();
                async move { client.get_output_status(&op.txid, op.vout as _).await }
            })
            .collect::<FuturesOrdered<_>>();

        if handles.is_empty() {
            break;
        }

        for op_status in handles.try_collect::<Vec<_>>().await?.into_iter().flatten() {
            let spend_txid = match op_status.txid {
                Some(txid) => txid,
                None => continue,
            };
            if !inserted_txs.contains(&spend_txid) {
                missing_txs.push(spend_txid);
            }
            if let Some(spend_status) = op_status.status {
                insert_anchor_from_status(&mut update, spend_txid, spend_status);
            }
        }
    }

    update
        .extend(fetch_txs_with_txids(client, inserted_txs, missing_txs, parallel_requests).await?);
    Ok(update)
}

#[cfg(test)]
mod test {
    use std::{collections::BTreeSet, time::Duration};

    use bdk_chain::{
        bitcoin::{hashes::Hash, Txid},
        local_chain::LocalChain,
        BlockId,
    };
    use bdk_core::ConfirmationBlockTime;
    use bdk_testenv::{anyhow, bitcoincore_rpc::RpcApi, TestEnv};
    use esplora_client::Builder;

    use crate::async_ext::{chain_update, fetch_latest_blocks};

    macro_rules! h {
        ($index:literal) => {{
            bdk_chain::bitcoin::hashes::Hash::hash($index.as_bytes())
        }};
    }

    /// Ensure that update does not remove heights (from original), and all anchor heights are included.
    #[tokio::test]
    pub async fn test_finalize_chain_update() -> anyhow::Result<()> {
        struct TestCase<'a> {
            #[allow(dead_code)]
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

        for t in test_cases.into_iter() {
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
                // force `chain_update_blocking` to add all checkpoints in `t.initial_cps`
                let anchors = t
                    .initial_cps
                    .iter()
                    .map(|&height| -> anyhow::Result<_> {
                        Ok((
                            ConfirmationBlockTime {
                                block_id: BlockId {
                                    height,
                                    hash: env.bitcoind.client.get_block_hash(height as _)?,
                                },
                                confirmation_time: height as _,
                            },
                            Txid::all_zeros(),
                        ))
                    })
                    .collect::<anyhow::Result<BTreeSet<_>>>()?;
                let update = chain_update(
                    &client,
                    &fetch_latest_blocks(&client).await?,
                    &chain.tip(),
                    &anchors,
                )
                .await?;
                chain.apply_update(update)?;
                chain
            };

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
                let anchors = t
                    .anchors
                    .iter()
                    .map(|&(height, txid)| -> anyhow::Result<_> {
                        Ok((
                            ConfirmationBlockTime {
                                block_id: BlockId {
                                    height,
                                    hash: env.bitcoind.client.get_block_hash(height as _)?,
                                },
                                confirmation_time: height as _,
                            },
                            txid,
                        ))
                    })
                    .collect::<anyhow::Result<_>>()?;
                chain_update(
                    &client,
                    &fetch_latest_blocks(&client).await?,
                    &local_chain.tip(),
                    &anchors,
                )
                .await?
            };

            // apply update
            let mut updated_local_chain = local_chain.clone();
            updated_local_chain.apply_update(update)?;

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
