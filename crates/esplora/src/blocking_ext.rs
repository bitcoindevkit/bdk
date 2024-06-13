use std::collections::BTreeSet;
use std::thread::JoinHandle;
use std::usize;

use bdk_chain::collections::BTreeMap;
use bdk_chain::spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult};
use bdk_chain::{
    bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, TxOut, Txid},
    local_chain::CheckPoint,
    BlockId, ConfirmationTimeHeightAnchor, TxGraph,
};
use bdk_chain::{Anchor, Indexed};
use esplora_client::TxStatus;

use crate::anchor_from_status;

/// [`esplora_client::Error`]
pub type Error = Box<esplora_client::Error>;

/// Trait to extend the functionality of [`esplora_client::BlockingClient`].
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
pub trait EsploraExt {
    /// Scan keychain scripts for transactions against Esplora, returning an update that can be
    /// applied to the receiving structures.
    ///
    /// - `request`: struct with data required to perform a spk-based blockchain client full scan,
    ///              see [`FullScanRequest`]
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
    fn full_scan<K: Ord + Clone>(
        &self,
        request: FullScanRequest<K>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error>;

    /// Sync a set of scripts with the blockchain (via an Esplora client) for the data
    /// specified and return a [`TxGraph`].
    ///
    /// - `request`: struct with data required to perform a spk-based blockchain client sync, see
    ///              [`SyncRequest`]
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`full_scan`]: EsploraExt::full_scan
    fn sync(&self, request: SyncRequest, parallel_requests: usize) -> Result<SyncResult, Error>;
}

impl EsploraExt for esplora_client::BlockingClient {
    fn full_scan<K: Ord + Clone>(
        &self,
        request: FullScanRequest<K>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error> {
        let latest_blocks = fetch_latest_blocks(self)?;
        let (graph_update, last_active_indices) = full_scan_for_index_and_graph_blocking(
            self,
            request.spks_by_keychain,
            stop_gap,
            parallel_requests,
        )?;
        let chain_update = chain_update(
            self,
            &latest_blocks,
            &request.chain_tip,
            graph_update.all_anchors(),
        )?;
        Ok(FullScanResult {
            chain_update,
            graph_update,
            last_active_indices,
        })
    }

    fn sync(&self, request: SyncRequest, parallel_requests: usize) -> Result<SyncResult, Error> {
        let latest_blocks = fetch_latest_blocks(self)?;
        let graph_update = sync_for_index_and_graph_blocking(
            self,
            request.spks,
            request.txids,
            request.outpoints,
            parallel_requests,
        )?;
        let chain_update = chain_update(
            self,
            &latest_blocks,
            &request.chain_tip,
            graph_update.all_anchors(),
        )?;
        Ok(SyncResult {
            chain_update,
            graph_update,
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
    let &tip_height = latest_blocks
        .keys()
        .last()
        .expect("must have atleast one entry");
    if height > tip_height {
        return Ok(None);
    }

    Ok(Some(client.get_block_hash(height)?))
}

/// Create the [`local_chain::Update`].
///
/// We want to have a corresponding checkpoint per anchor height. However, checkpoints fetched
/// should not surpass `latest_blocks`.
fn chain_update<A: Anchor>(
    client: &esplora_client::BlockingClient,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    local_tip: &CheckPoint,
    anchors: &BTreeSet<(A, Txid)>,
) -> Result<CheckPoint, Error> {
    let mut point_of_agreement = None;
    let mut conflicts = vec![];
    for local_cp in local_tip.iter() {
        let remote_hash = match fetch_block(client, latest_blocks, local_cp.height())? {
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

    for anchor in anchors {
        let height = anchor.0.anchor_block().height;
        if tip.get(height).is_none() {
            let hash = match fetch_block(client, latest_blocks, height)? {
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

/// This performs a full scan to get an update for the [`TxGraph`] and
/// [`KeychainTxOutIndex`](bdk_chain::keychain::KeychainTxOutIndex).
fn full_scan_for_index_and_graph_blocking<K: Ord + Clone>(
    client: &esplora_client::BlockingClient,
    keychain_spks: BTreeMap<K, impl IntoIterator<Item = Indexed<ScriptBuf>>>,
    stop_gap: usize,
    parallel_requests: usize,
) -> Result<(TxGraph<ConfirmationTimeHeightAnchor>, BTreeMap<K, u32>), Error> {
    type TxsOfSpkIndex = (u32, Vec<esplora_client::Tx>);
    let parallel_requests = Ord::max(parallel_requests, 1);
    let mut tx_graph = TxGraph::<ConfirmationTimeHeightAnchor>::default();
    let mut last_active_indices = BTreeMap::<K, u32>::new();

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
                        let client = client.clone();
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
                    let _ = tx_graph.insert_tx(tx.to_tx());
                    if let Some(anchor) = anchor_from_status(&tx.status) {
                        let _ = tx_graph.insert_anchor(tx.txid, anchor);
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
                        let _ = tx_graph.insert_txout(outpoint, txout);
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
            last_active_indices.insert(keychain, last_active_index);
        }
    }

    Ok((tx_graph, last_active_indices))
}

fn sync_for_index_and_graph_blocking(
    client: &esplora_client::BlockingClient,
    misc_spks: impl IntoIterator<Item = ScriptBuf>,
    txids: impl IntoIterator<Item = Txid>,
    outpoints: impl IntoIterator<Item = OutPoint>,
    parallel_requests: usize,
) -> Result<TxGraph<ConfirmationTimeHeightAnchor>, Error> {
    let (mut tx_graph, _) = full_scan_for_index_and_graph_blocking(
        client,
        {
            let mut keychains = BTreeMap::new();
            keychains.insert(
                (),
                misc_spks
                    .into_iter()
                    .enumerate()
                    .map(|(i, spk)| (i as u32, spk)),
            );
            keychains
        },
        usize::MAX,
        parallel_requests,
    )?;

    let mut txids = txids.into_iter();
    loop {
        let handles = txids
            .by_ref()
            .take(parallel_requests)
            .filter(|&txid| tx_graph.get_tx(txid).is_none())
            .map(|txid| {
                std::thread::spawn({
                    let client = client.clone();
                    move || {
                        client
                            .get_tx_status(&txid)
                            .map_err(Box::new)
                            .map(|s| (txid, s))
                    }
                })
            })
            .collect::<Vec<JoinHandle<Result<(Txid, TxStatus), Error>>>>();

        if handles.is_empty() {
            break;
        }

        for handle in handles {
            let (txid, status) = handle.join().expect("thread must not panic")?;
            if let Some(anchor) = anchor_from_status(&status) {
                let _ = tx_graph.insert_anchor(txid, anchor);
            }
        }
    }

    for op in outpoints {
        if tx_graph.get_tx(op.txid).is_none() {
            if let Some(tx) = client.get_tx(&op.txid)? {
                let _ = tx_graph.insert_tx(tx);
            }
            let status = client.get_tx_status(&op.txid)?;
            if let Some(anchor) = anchor_from_status(&status) {
                let _ = tx_graph.insert_anchor(op.txid, anchor);
            }
        }

        if let Some(op_status) = client.get_output_status(&op.txid, op.vout as _)? {
            if let Some(txid) = op_status.txid {
                if tx_graph.get_tx(txid).is_none() {
                    if let Some(tx) = client.get_tx(&txid)? {
                        let _ = tx_graph.insert_tx(tx);
                    }
                    let status = client.get_tx_status(&txid)?;
                    if let Some(anchor) = anchor_from_status(&status) {
                        let _ = tx_graph.insert_anchor(txid, anchor);
                    }
                }
            }
        }
    }

    Ok(tx_graph)
}

#[cfg(test)]
mod test {
    use crate::blocking_ext::{chain_update, fetch_latest_blocks};
    use bdk_chain::bitcoin::hashes::Hash;
    use bdk_chain::bitcoin::Txid;
    use bdk_chain::local_chain::LocalChain;
    use bdk_chain::BlockId;
    use bdk_testenv::{anyhow, bitcoincore_rpc::RpcApi, TestEnv};
    use esplora_client::{BlockHash, Builder};
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    macro_rules! h {
        ($index:literal) => {{
            bdk_chain::bitcoin::hashes::Hash::hash($index.as_bytes())
        }};
    }

    macro_rules! local_chain {
        [ $(($height:expr, $block_hash:expr)), * ] => {{
            #[allow(unused_mut)]
            bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $block_hash).into()),*].into_iter().collect())
                .expect("chain must have genesis block")
        }};
    }

    /// Ensure that update does not remove heights (from original), and all anchor heights are included.
    #[test]
    pub fn test_finalize_chain_update() -> anyhow::Result<()> {
        struct TestCase<'a> {
            name: &'a str,
            /// Initial blockchain height to start the env with.
            initial_env_height: u32,
            /// Initial checkpoint heights to start with in the local chain.
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
                let (mut chain, _) = LocalChain::from_genesis_hash(env.genesis_hash()?);
                // force `chain_update_blocking` to add all checkpoints in `t.initial_cps`
                let anchors = t
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
                let update = chain_update(
                    &client,
                    &fetch_latest_blocks(&client)?,
                    &chain.tip(),
                    &anchors,
                )?;
                chain.apply_update(update)?;
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
                            BlockId {
                                height,
                                hash: env.bitcoind.client.get_block_hash(height as _)?,
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
            println!("Case {}: {}", i, t.name);
            let mut chain = t.chain;

            let mock_anchors = t
                .request_heights
                .iter()
                .map(|&h| {
                    let anchor_blockhash: BlockHash = bdk_chain::bitcoin::hashes::Hash::hash(
                        &format!("hash_at_height_{}", h).into_bytes(),
                    );
                    let txid: Txid = bdk_chain::bitcoin::hashes::Hash::hash(
                        &format!("txid_at_height_{}", h).into_bytes(),
                    );
                    let anchor = BlockId {
                        height: h,
                        hash: anchor_blockhash,
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
