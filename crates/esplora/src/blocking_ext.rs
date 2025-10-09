use bdk_core::collections::{BTreeMap, BTreeSet, HashSet};
use bdk_core::spk_client::{
    FullScanRequest, FullScanResponse, SpkWithExpectedTxids, SyncRequest, SyncResponse,
};
use bdk_core::{
    bitcoin::{BlockHash, OutPoint, Txid},
    BlockId, CheckPoint, ConfirmationBlockTime, Indexed, TxUpdate,
};
use esplora_client::{OutputStatus, Tx};
use std::thread::JoinHandle;

use crate::{insert_anchor_or_seen_at_from_status, insert_prevouts};

/// [`esplora_client::Error`]
pub type Error = Box<esplora_client::Error>;

/// Trait to extend the functionality of [`esplora_client::BlockingClient`].
///
/// Refer to [crate-level documentation](crate) for more.
pub trait EsploraExt {
    /// Scan keychain scripts for transactions against Esplora, returning an update that can be
    /// applied to the receiving structures.
    ///
    /// `request` provides the data required to perform a script-pubkey-based full scan
    /// (see [`FullScanRequest`]). The full scan for each keychain (`K`) stops after a gap of
    /// `stop_gap` script pubkeys with no associated transactions. `parallel_requests` specifies
    /// the maximum number of HTTP requests to make in parallel.
    ///
    /// Refer to [crate-level docs](crate) for more.
    fn full_scan<K: Ord + Clone, R: Into<FullScanRequest<K>>>(
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
    fn sync<I: 'static, R: Into<SyncRequest<I>>>(
        &self,
        request: R,
        parallel_requests: usize,
    ) -> Result<SyncResponse, Error>;
}

impl EsploraExt for esplora_client::BlockingClient {
    fn full_scan<K: Ord + Clone, R: Into<FullScanRequest<K>>>(
        &self,
        request: R,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResponse<K>, Error> {
        let mut request: FullScanRequest<K> = request.into();
        let start_time = request.start_time();

        let chain_tip = request.chain_tip();
        let latest_blocks = if chain_tip.is_some() {
            Some(fetch_latest_blocks(self)?)
        } else {
            None
        };

        let mut tx_update = TxUpdate::default();
        let mut inserted_txs = HashSet::<Txid>::new();
        let mut last_active_indices = BTreeMap::<K, u32>::new();
        for keychain in request.keychains() {
            let keychain_spks = request
                .iter_spks(keychain.clone())
                .map(|(spk_i, spk)| (spk_i, spk.into()));
            let (update, last_active_index) = fetch_txs_with_keychain_spks(
                self,
                start_time,
                &mut inserted_txs,
                keychain_spks,
                stop_gap,
                parallel_requests,
            )?;
            tx_update.extend(update);
            if let Some(last_active_index) = last_active_index {
                last_active_indices.insert(keychain, last_active_index);
            }
        }

        let chain_update = match (chain_tip, latest_blocks) {
            (Some(chain_tip), Some(latest_blocks)) => Some(chain_update(
                self,
                &latest_blocks,
                &chain_tip,
                &tx_update.anchors,
            )?),
            _ => None,
        };

        Ok(FullScanResponse {
            chain_update,
            tx_update,
            last_active_indices,
        })
    }

    fn sync<I: 'static, R: Into<SyncRequest<I>>>(
        &self,
        request: R,
        parallel_requests: usize,
    ) -> Result<SyncResponse, Error> {
        let mut request: SyncRequest<I> = request.into();
        let start_time = request.start_time();

        let chain_tip = request.chain_tip();
        let latest_blocks = if chain_tip.is_some() {
            Some(fetch_latest_blocks(self)?)
        } else {
            None
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        let mut inserted_txs = HashSet::<Txid>::new();
        tx_update.extend(fetch_txs_with_spks(
            self,
            start_time,
            &mut inserted_txs,
            request.iter_spks_with_expected_txids(),
            parallel_requests,
        )?);
        tx_update.extend(fetch_txs_with_txids(
            self,
            start_time,
            &mut inserted_txs,
            request.iter_txids(),
            parallel_requests,
        )?);
        tx_update.extend(fetch_txs_with_outpoints(
            self,
            start_time,
            &mut inserted_txs,
            request.iter_outpoints(),
            parallel_requests,
        )?);

        let chain_update = match (chain_tip, latest_blocks) {
            (Some(chain_tip), Some(latest_blocks)) => Some(chain_update(
                self,
                &latest_blocks,
                &chain_tip,
                &tx_update.anchors,
            )?),
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
fn fetch_latest_blocks(
    client: &esplora_client::BlockingClient,
) -> Result<BTreeMap<u32, BlockHash>, Error> {
    Ok(client
        .get_blocks(None)?
        .into_iter()
        .map(|b| (b.time.height, b.id))
        .collect())
}

/// Used instead of [`esplora_client::BlockingClient::get_block_hash`].
///
/// This first checks the previously fetched `latest_blocks` before fetching from Esplora again.
fn fetch_block(
    client: &esplora_client::BlockingClient,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    height: u32,
) -> Result<Option<BlockHash>, Error> {
    if let Some(&hash) = latest_blocks.get(&height) {
        return Ok(Some(hash));
    }

    // We avoid fetching blocks higher than previously fetched `latest_blocks` as the local chain
    // tip is used to signal for the last-synced-up-to-height.
    match latest_blocks.keys().last().copied() {
        None => {
            debug_assert!(false, "`latest_blocks` should not be empty");
            return Ok(None);
        }
        Some(tip_height) => {
            if height > tip_height {
                return Ok(None);
            }
        }
    }

    Ok(Some(client.get_block_hash(height)?))
}

/// Create the [`local_chain::Update`].
///
/// We want to have a corresponding checkpoint per anchor height. However, checkpoints fetched
/// should not surpass `latest_blocks`.
fn chain_update(
    client: &esplora_client::BlockingClient,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    local_tip: &CheckPoint<BlockHash>,
    anchors: &BTreeSet<(ConfirmationBlockTime, Txid)>,
) -> Result<CheckPoint<BlockHash>, Error> {
    let mut point_of_agreement = None;
    let mut local_cp_hash = local_tip.hash();
    let mut conflicts = vec![];

    for local_cp in local_tip.iter() {
        let remote_hash = match fetch_block(client, latest_blocks, local_cp.height())? {
            Some(hash) => hash,
            None => continue,
        };
        if remote_hash == local_cp.hash() {
            point_of_agreement = Some(local_cp);
            break;
        }
        local_cp_hash = local_cp.hash();
        // It is not strictly necessary to include all the conflicted heights (we do need the
        // first one) but it seems prudent to make sure the updated chain's heights are a
        // superset of the existing chain after update.
        conflicts.push(BlockId {
            height: local_cp.height(),
            hash: remote_hash,
        });
    }

    let mut tip = match point_of_agreement {
        Some(tip) => tip,
        None => {
            return Err(Box::new(esplora_client::Error::HeaderHashNotFound(
                local_cp_hash,
            )));
        }
    };

    tip = tip
        .extend(conflicts.into_iter().rev().map(|b| (b.height, b.hash)))
        .map_err(|_| Box::new(esplora_client::Error::InvalidResponse))?;

    for (anchor, _) in anchors {
        let height = anchor.block_id.height;
        if tip.get(height).is_none() {
            let hash = match fetch_block(client, latest_blocks, height)? {
                Some(hash) => hash,
                None => continue,
            };
            tip = tip.insert(height, hash);
        }
    }

    // insert the most recent blocks at the tip to make sure we update the tip and make the update
    // robust.
    for (&height, &hash) in latest_blocks.iter() {
        tip = tip.insert(height, hash);
    }

    Ok(tip)
}

fn fetch_txs_with_keychain_spks<I: Iterator<Item = Indexed<SpkWithExpectedTxids>>>(
    client: &esplora_client::BlockingClient,
    start_time: u64,
    inserted_txs: &mut HashSet<Txid>,
    mut keychain_spks: I,
    stop_gap: usize,
    parallel_requests: usize,
) -> Result<(TxUpdate<ConfirmationBlockTime>, Option<u32>), Error> {
    type TxsOfSpkIndex = (u32, Vec<esplora_client::Tx>, HashSet<Txid>);

    let mut update = TxUpdate::<ConfirmationBlockTime>::default();
    let mut last_index = Option::<u32>::None;
    let mut last_active_index = Option::<u32>::None;

    loop {
        let handles = keychain_spks
            .by_ref()
            .take(parallel_requests)
            .map(|(spk_index, spk)| {
                let client = client.clone();
                let expected_txids = spk.expected_txids;
                let spk = spk.spk;
                std::thread::spawn(move || -> Result<TxsOfSpkIndex, Error> {
                    let mut last_txid = None;
                    let mut spk_txs = Vec::new();
                    loop {
                        let txs = client.scripthash_txs(&spk, last_txid)?;
                        let tx_count = txs.len();
                        last_txid = txs.last().map(|tx| tx.txid);
                        spk_txs.extend(txs);
                        if tx_count < 25 {
                            break;
                        }
                    }
                    let got_txids = spk_txs.iter().map(|tx| tx.txid).collect::<HashSet<_>>();
                    let evicted_txids = expected_txids
                        .difference(&got_txids)
                        .copied()
                        .collect::<HashSet<_>>();
                    Ok((spk_index, spk_txs, evicted_txids))
                })
            })
            .collect::<Vec<JoinHandle<Result<TxsOfSpkIndex, Error>>>>();

        if handles.is_empty() {
            if last_index.is_none() {
                return Err(Box::new(esplora_client::Error::InvalidResponse));
            }
            break;
        }

        for handle in handles {
            let handle_result = handle
                .join()
                .map_err(|_| Box::new(esplora_client::Error::InvalidResponse))?;
            let (index, txs, evicted) = handle_result?;
            last_index = Some(index);
            if !txs.is_empty() {
                last_active_index = Some(index);
            }
            for tx in txs {
                if inserted_txs.insert(tx.txid) {
                    update.txs.push(tx.to_tx().into());
                }
                insert_anchor_or_seen_at_from_status(&mut update, start_time, tx.txid, tx.status);
                insert_prevouts(&mut update, tx.vin);
            }
            update
                .evicted_ats
                .extend(evicted.into_iter().map(|txid| (txid, start_time)));
        }

        let last_index =
            last_index.ok_or_else(|| Box::new(esplora_client::Error::InvalidResponse))?;
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
/// Unlike with [`EsploraExt::fetch_txs_with_keychain_spks`], `spks` must be *bounded* as all
/// contained scripts will be scanned. `parallel_requests` specifies the maximum number of HTTP
/// requests to make in parallel.
///
/// Refer to [crate-level docs](crate) for more.
fn fetch_txs_with_spks<I: IntoIterator<Item = SpkWithExpectedTxids>>(
    client: &esplora_client::BlockingClient,
    start_time: u64,
    inserted_txs: &mut HashSet<Txid>,
    spks: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error> {
    fetch_txs_with_keychain_spks(
        client,
        start_time,
        inserted_txs,
        spks.into_iter().enumerate().map(|(i, spk)| (i as u32, spk)),
        usize::MAX,
        parallel_requests,
    )
    .map(|(update, _)| update)
}

/// Fetch transactions and associated [`ConfirmationBlockTime`]s by scanning `txids`
/// against Esplora.
///
/// `parallel_requests` specifies the maximum number of HTTP requests to make in parallel.
///
/// Refer to [crate-level docs](crate) for more.
fn fetch_txs_with_txids<I: IntoIterator<Item = Txid>>(
    client: &esplora_client::BlockingClient,
    start_time: u64,
    inserted_txs: &mut HashSet<Txid>,
    txids: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error> {
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
                std::thread::spawn(move || {
                    client
                        .get_tx_info(&txid)
                        .map_err(Box::new)
                        .map(|t| (txid, t))
                })
            })
            .collect::<Vec<JoinHandle<Result<(Txid, Option<Tx>), Error>>>>();

        if handles.is_empty() {
            break;
        }

        for handle in handles {
            let handle_result = handle
                .join()
                .map_err(|_| Box::new(esplora_client::Error::InvalidResponse))?;
            let (txid, tx_info) = handle_result?;
            if let Some(tx_info) = tx_info {
                if inserted_txs.insert(txid) {
                    update.txs.push(tx_info.to_tx().into());
                }
                insert_anchor_or_seen_at_from_status(&mut update, start_time, txid, tx_info.status);
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
fn fetch_txs_with_outpoints<I: IntoIterator<Item = OutPoint>>(
    client: &esplora_client::BlockingClient,
    start_time: u64,
    inserted_txs: &mut HashSet<Txid>,
    outpoints: I,
    parallel_requests: usize,
) -> Result<TxUpdate<ConfirmationBlockTime>, Error> {
    let outpoints = outpoints.into_iter().collect::<Vec<_>>();
    let mut update = TxUpdate::<ConfirmationBlockTime>::default();

    // make sure txs exists in graph and tx statuses are updated
    // TODO: We should maintain a tx cache (like we do with Electrum).
    update.extend(fetch_txs_with_txids(
        client,
        start_time,
        inserted_txs,
        outpoints.iter().map(|op| op.txid),
        parallel_requests,
    )?);

    // get outpoint spend-statuses
    let mut outpoints = outpoints.into_iter();
    let mut missing_txs = Vec::<Txid>::with_capacity(outpoints.len());
    loop {
        let handles = outpoints
            .by_ref()
            .take(parallel_requests)
            .map(|op| {
                let client = client.clone();
                std::thread::spawn(move || {
                    client
                        .get_output_status(&op.txid, op.vout as _)
                        .map_err(Box::new)
                })
            })
            .collect::<Vec<JoinHandle<Result<Option<OutputStatus>, Error>>>>();

        if handles.is_empty() {
            break;
        }

        for handle in handles {
            let handle_result = handle
                .join()
                .map_err(|_| Box::new(esplora_client::Error::InvalidResponse))?;
            if let Some(op_status) = handle_result? {
                let spend_txid = match op_status.txid {
                    Some(txid) => txid,
                    None => continue,
                };
                if !inserted_txs.contains(&spend_txid) {
                    missing_txs.push(spend_txid);
                }
                if let Some(spend_status) = op_status.status {
                    insert_anchor_or_seen_at_from_status(
                        &mut update,
                        start_time,
                        spend_txid,
                        spend_status,
                    );
                }
            }
        }
    }

    update.extend(fetch_txs_with_txids(
        client,
        start_time,
        inserted_txs,
        missing_txs,
        parallel_requests,
    )?);
    Ok(update)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use crate::blocking_ext::{chain_update, fetch_latest_blocks, Error};
    use bdk_chain::bitcoin;
    use bdk_chain::bitcoin::hashes::Hash;
    use bdk_chain::bitcoin::Txid;
    use bdk_chain::local_chain::LocalChain;
    use bdk_chain::BlockId;
    use bdk_core::ConfirmationBlockTime;
    use bdk_testenv::{anyhow, bitcoincore_rpc::RpcApi, TestEnv};
    use esplora_client::{BlockHash, Builder};
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    macro_rules! h {
        ($index:literal) => {{
            bdk_chain::bitcoin::hashes::Hash::hash($index.as_bytes())
        }};
    }

    #[test]
    fn thread_join_panic_maps_to_error() {
        let handle = std::thread::spawn(|| -> Result<(), Error> {
            panic!("expected panic for test coverage");
        });

        let res = (|| -> Result<(), Error> {
            let handle_result = handle
                .join()
                .map_err(|_| Box::new(esplora_client::Error::InvalidResponse))?;
            handle_result?;
            Ok(())
        })();

        assert!(matches!(
            *res.unwrap_err(),
            esplora_client::Error::InvalidResponse
        ));
    }

    #[test]
    fn ensure_last_index_none_returns_error() {
        let last_index: Option<u32> = None;
        let err = last_index
            .ok_or_else(|| Box::new(esplora_client::Error::InvalidResponse))
            .unwrap_err();
        assert!(matches!(*err, esplora_client::Error::InvalidResponse));
    }

    macro_rules! local_chain {
        [ $(($height:expr, $block_hash:expr)), * ] => {{
            #[allow(unused_mut)]
            bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $block_hash).into()),*].into_iter().collect())
                .expect("chain must have genesis block")
        }};
    }

    // Test that `chain_update` fails due to wrong network.
    #[test]
    fn test_chain_update_wrong_network_error() -> anyhow::Result<()> {
        let env = TestEnv::new()?;
        let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_blocking();
        let initial_height = env.rpc_client().get_block_count()? as u32;

        let mine_to = 16;
        let _ = env.mine_blocks((mine_to - initial_height) as usize, None)?;
        while client.get_height()? < mine_to {
            std::thread::sleep(Duration::from_millis(64));
        }
        let latest_blocks = fetch_latest_blocks(&client)?;
        assert!(!latest_blocks.is_empty());
        assert_eq!(latest_blocks.keys().last(), Some(&mine_to));

        let genesis_hash =
            bitcoin::constants::genesis_block(bitcoin::Network::Testnet4).block_hash();
        let cp = bdk_chain::CheckPoint::new(0, genesis_hash);

        let anchors = BTreeSet::new();
        let res = chain_update(&client, &latest_blocks, &cp, &anchors);
        use esplora_client::Error;
        assert!(
            matches!(*res.unwrap_err(), Error::HeaderHashNotFound(hash) if hash == genesis_hash),
            "`chain_update` should error if it can't connect to the local CP",
        );

        Ok(())
    }

    /// Ensure that update does not remove heights (from original), and all anchor heights are
    /// included.
    #[test]
    pub fn test_finalize_chain_update() -> anyhow::Result<()> {
        struct TestCase<'a> {
            #[allow(dead_code)]
            name: &'a str,
            /// Initial blockchain height to start the env with.
            initial_env_height: u32,
            /// Initial checkpoint heights to start with in the local chain.
            initial_cps: &'a [u32],
            /// The final blockchain height of the env.
            final_env_height: u32,
            /// The anchors to test with: `(height, txid)`. Only the height is provided as we can
            /// fetch the blockhash from the env.
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
            let client = Builder::new(base_url.as_str()).build_blocking();

            // set env to `initial_env_height`
            if let Some(to_mine) = t
                .initial_env_height
                .checked_sub(env.make_checkpoint_tip().height())
            {
                env.mine_blocks(to_mine as _, None)?;
            }
            while client.get_height()? < t.initial_env_height {
                std::thread::sleep(Duration::from_millis(10));
            }

            // craft initial `local_chain`
            let local_chain = {
                let (mut chain, _) = LocalChain::from_genesis(env.genesis_hash()?);
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
                    &fetch_latest_blocks(&client)?,
                    &chain.tip(),
                    &anchors,
                )?;
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
            while client.get_height()? < t.final_env_height {
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
                    &fetch_latest_blocks(&client)?,
                    &local_chain.tip(),
                    &anchors,
                )?
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

    #[test]
    fn update_local_chain() -> anyhow::Result<()> {
        const TIP_HEIGHT: u32 = 50;

        let env = TestEnv::new()?;
        let blocks = {
            let bitcoind_client = &env.bitcoind.client;
            assert_eq!(bitcoind_client.get_block_count()?, 1);
            [
                (0, bitcoind_client.get_block_hash(0)?),
                (1, bitcoind_client.get_block_hash(1)?),
            ]
            .into_iter()
            .chain((2..).zip(env.mine_blocks((TIP_HEIGHT - 1) as usize, None)?))
            .collect::<BTreeMap<_, _>>()
        };
        // so new blocks can be seen by Electrs
        let env = env.reset_electrsd()?;
        let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_blocking();

        struct TestCase {
            name: &'static str,
            /// Original local chain to start off with.
            chain: LocalChain,
            /// Heights of floating anchors. [`chain_update_blocking`] will request for checkpoints
            /// of these heights.
            request_heights: &'static [u32],
            /// The expected local chain result (heights only).
            exp_update_heights: &'static [u32],
        }

        let test_cases = [
            TestCase {
                name: "request_later_blocks",
                chain: local_chain![(0, blocks[&0]), (21, blocks[&21])],
                request_heights: &[22, 25, 28],
                exp_update_heights: &[21, 22, 25, 28],
            },
            TestCase {
                name: "request_prev_blocks",
                chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (5, blocks[&5])],
                request_heights: &[4],
                exp_update_heights: &[4, 5],
            },
            TestCase {
                name: "request_prev_blocks_2",
                chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (10, blocks[&10])],
                request_heights: &[4, 6],
                exp_update_heights: &[4, 6, 10],
            },
            TestCase {
                name: "request_later_and_prev_blocks",
                chain: local_chain![(0, blocks[&0]), (7, blocks[&7]), (11, blocks[&11])],
                request_heights: &[8, 9, 15],
                exp_update_heights: &[8, 9, 11, 15],
            },
            TestCase {
                name: "request_tip_only",
                chain: local_chain![(0, blocks[&0]), (5, blocks[&5]), (49, blocks[&49])],
                request_heights: &[TIP_HEIGHT],
                exp_update_heights: &[49],
            },
            TestCase {
                name: "request_nothing",
                chain: local_chain![(0, blocks[&0]), (13, blocks[&13]), (23, blocks[&23])],
                request_heights: &[],
                exp_update_heights: &[23],
            },
            TestCase {
                name: "request_nothing_during_reorg",
                chain: local_chain![(0, blocks[&0]), (13, blocks[&13]), (23, h!("23"))],
                request_heights: &[],
                exp_update_heights: &[13, 23],
            },
            TestCase {
                name: "request_nothing_during_reorg_2",
                chain: local_chain![
                    (0, blocks[&0]),
                    (21, blocks[&21]),
                    (22, h!("22")),
                    (23, h!("23"))
                ],
                request_heights: &[],
                exp_update_heights: &[21, 22, 23],
            },
            TestCase {
                name: "request_prev_blocks_during_reorg",
                chain: local_chain![
                    (0, blocks[&0]),
                    (21, blocks[&21]),
                    (22, h!("22")),
                    (23, h!("23"))
                ],
                request_heights: &[17, 20],
                exp_update_heights: &[17, 20, 21, 22, 23],
            },
            TestCase {
                name: "request_later_blocks_during_reorg",
                chain: local_chain![
                    (0, blocks[&0]),
                    (9, blocks[&9]),
                    (22, h!("22")),
                    (23, h!("23"))
                ],
                request_heights: &[25, 27],
                exp_update_heights: &[9, 22, 23, 25, 27],
            },
            TestCase {
                name: "request_later_blocks_during_reorg_2",
                chain: local_chain![(0, blocks[&0]), (9, h!("9"))],
                request_heights: &[10],
                exp_update_heights: &[0, 9, 10],
            },
            TestCase {
                name: "request_later_and_prev_blocks_during_reorg",
                chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (9, h!("9"))],
                request_heights: &[8, 11],
                exp_update_heights: &[1, 8, 9, 11],
            },
        ];

        for (i, t) in test_cases.into_iter().enumerate() {
            let mut chain = t.chain;

            let mock_anchors = t
                .request_heights
                .iter()
                .map(|&h| {
                    let anchor_blockhash: BlockHash = bdk_chain::bitcoin::hashes::Hash::hash(
                        &format!("hash_at_height_{h}").into_bytes(),
                    );
                    let txid: Txid = bdk_chain::bitcoin::hashes::Hash::hash(
                        &format!("txid_at_height_{h}").into_bytes(),
                    );
                    let anchor = ConfirmationBlockTime {
                        block_id: BlockId {
                            height: h,
                            hash: anchor_blockhash,
                        },
                        confirmation_time: h as _,
                    };
                    (anchor, txid)
                })
                .collect::<BTreeSet<_>>();
            let chain_update = chain_update(
                &client,
                &fetch_latest_blocks(&client)?,
                &chain.tip(),
                &mock_anchors,
            )?;

            let update_blocks = chain_update
                .iter()
                .map(|cp| cp.block_id())
                .collect::<BTreeSet<_>>();

            let exp_update_blocks = t
                .exp_update_heights
                .iter()
                .map(|&height| {
                    let hash = blocks[&height];
                    BlockId { height, hash }
                })
                .chain(
                    // Electrs Esplora `get_block` call fetches 10 blocks which is included in the
                    // update
                    blocks
                        .range(TIP_HEIGHT - 9..)
                        .map(|(&height, &hash)| BlockId { height, hash }),
                )
                .collect::<BTreeSet<_>>();

            assert!(
                update_blocks.is_superset(&exp_update_blocks),
                "[{}:{}] unexpected update",
                i,
                t.name
            );

            let _ = chain
                .apply_update(chain_update)
                .unwrap_or_else(|err| panic!("[{}:{}] update failed to apply: {}", i, t.name, err));

            // all requested heights must exist in the final chain
            for height in t.request_heights {
                let exp_blockhash = blocks.get(height).expect("block must exist in bitcoind");
                assert_eq!(
                    chain.get(*height).map(|cp| cp.hash()),
                    Some(*exp_blockhash),
                    "[{}:{}] block {}:{} must exist in final chain",
                    i,
                    t.name,
                    height,
                    exp_blockhash
                );
            }
        }

        Ok(())
    }
}
