use async_trait::async_trait;
use bdk_core::collections::{BTreeMap, BTreeSet, HashSet};
use bdk_core::spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult};
use bdk_core::{
    bitcoin::{BlockHash, OutPoint, ScriptBuf, Txid},
    BlockId, CheckPoint, ConfirmationBlockTime, Indexed, TxUpdate,
};
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
    ) -> Result<FullScanResult<K>, Error>;

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
    ) -> Result<SyncResult, Error>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl EsploraAsyncExt for esplora_client::AsyncClient {
    async fn full_scan<K: Ord + Clone + Send, R: Into<FullScanRequest<K>> + Send>(
        &self,
        request: R,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error> {
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
            let (update, last_active_index) = fetch_txs_with_keychain_spks(
                self,
                &mut inserted_txs,
                keychain_spks,
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

        Ok(FullScanResult {
            chain_update,
            tx_update,
            last_active_indices,
        })
    }

    async fn sync<I: Send, R: Into<SyncRequest<I>> + Send>(
        &self,
        request: R,
        parallel_requests: usize,
    ) -> Result<SyncResult, Error> {
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
                request.iter_spks(),
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

        Ok(SyncResult {
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
async fn fetch_latest_blocks(
    client: &esplora_client::AsyncClient,
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
async fn fetch_block(
    client: &esplora_client::AsyncClient,
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
async fn chain_update(
    client: &esplora_client::AsyncClient,
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
async fn fetch_txs_with_keychain_spks<I: Iterator<Item = Indexed<ScriptBuf>> + Send>(
    client: &esplora_client::AsyncClient,
    inserted_txs: &mut HashSet<Txid>,
    mut keychain_spks: I,
    stop_gap: usize,
    parallel_requests: usize,
) -> Result<(TxUpdate<ConfirmationBlockTime>, Option<u32>), Error> {
    type TxsOfSpkIndex = (u32, Vec<esplora_client::Tx>);

    let mut update = TxUpdate::<ConfirmationBlockTime>::default();
    let mut last_index = Option::<u32>::None;
    let mut last_active_index = Option::<u32>::None;

    loop {
        let handles = keychain_spks
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
async fn fetch_txs_with_spks<I: IntoIterator<Item = ScriptBuf> + Send>(
    client: &esplora_client::AsyncClient,
    inserted_txs: &mut HashSet<Txid>,
    spks: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error>
where
    I::IntoIter: Send,
{
    fetch_txs_with_keychain_spks(
        client,
        inserted_txs,
        spks.into_iter().enumerate().map(|(i, spk)| (i as u32, spk)),
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
async fn fetch_txs_with_txids<I: IntoIterator<Item = Txid> + Send>(
    client: &esplora_client::AsyncClient,
    inserted_txs: &mut HashSet<Txid>,
    txids: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error>
where
    I::IntoIter: Send,
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
async fn fetch_txs_with_outpoints<I: IntoIterator<Item = OutPoint> + Send>(
    client: &esplora_client::AsyncClient,
    inserted_txs: &mut HashSet<Txid>,
    outpoints: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error>
where
    I::IntoIter: Send,
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
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    use bdk_chain::bitcoin::{constants, hashes::Hash, BlockHash, Network, Txid};
    use bdk_chain::{local_chain::LocalChain, BlockId};
    use bdk_core::ConfirmationBlockTime;
    use bdk_testenv::{anyhow, bitcoincore_rpc::RpcApi, TestEnv};
    use esplora_client::Builder;

    use crate::async_ext::{chain_update, fetch_latest_blocks};

    /// Ensure that update does not remove heights (from original), and all anchor heights are included.
    #[tokio::test]
    pub async fn test_finalize_chain_update() -> anyhow::Result<()> {
        let env = TestEnv::new()?;
        let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_async()?;
        let init_count: u32 = env.rpc_client().get_block_count()?.try_into()?;
        assert_eq!(init_count, 1);

        // mine blocks
        let final_env_height = 15;
        let to_mine = (final_env_height - init_count) as usize;
        let blocks: BTreeMap<u32, BlockHash> = (init_count + 1..=final_env_height)
            .zip(env.mine_blocks(to_mine, None)?)
            .collect();

        // wait for esplora client to catch up
        let dur = Duration::from_millis(64);
        while client.get_height().await? < final_env_height {
            std::thread::sleep(dur);
        }

        // initialize local chain
        let genesis_hash = constants::genesis_block(Network::Regtest).block_hash();
        let mut cp = LocalChain::from_genesis_hash(genesis_hash).0.tip();
        let local_chain_heights = vec![2, 4];
        for &height in local_chain_heights.iter() {
            let hash = blocks[&height];
            let block = BlockId { height, hash };
            cp = cp.insert(block);
        }

        // include these anchors when requesting a chain update. anchor 3 is behind
        // the local chain tip, and anchor 5 is ahead
        let mut anchors = BTreeSet::new();
        let anchor_heights = vec![3, 5];
        for &height in anchor_heights.iter() {
            let anchor = ConfirmationBlockTime {
                block_id: BlockId {
                    height,
                    hash: blocks[&height],
                },
                confirmation_time: height as u64,
            };
            anchors.insert((anchor, Txid::all_zeros()));
        }

        // fetch latest and get update
        let latest_blocks = fetch_latest_blocks(&client).await?;
        assert_eq!(latest_blocks.len(), 10);
        let update = chain_update(&client, &latest_blocks, &cp, &anchors).await?;
        assert_eq!(update.height(), final_env_height);

        // check update cp contains expected heights
        for height in local_chain_heights.into_iter().chain(anchor_heights) {
            let cp = update.get(height).expect("update must have height");
            assert_eq!(blocks[&cp.height()], cp.hash());
        }

        Ok(())
    }
}
