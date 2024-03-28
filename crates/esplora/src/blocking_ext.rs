use std::collections::BTreeSet;
use std::fmt::Debug;
use std::thread::JoinHandle;
use std::usize;

use bdk_chain::collections::btree_map;
use bdk_chain::collections::BTreeMap;
use bdk_chain::spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult};
use bdk_chain::Anchor;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, ScriptBuf, TxOut, Txid},
    local_chain::{self, CheckPoint},
    BlockId, ConfirmationTimeHeightAnchor, TxGraph,
};
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
    /// The full scan for each keychain stops after a gap of `stop_gap` script pubkeys with no
    /// associated transactions. `parallel_requests` specifies the max number of HTTP requests to
    /// make in parallel.
    ///
    /// [`LocalChain::tip`]: local_chain::LocalChain::tip
    fn full_scan<
        K: Ord + Clone + Send + Debug + 'static,
        I: Iterator<Item = (u32, ScriptBuf)> + Send,
    >(
        &self,
        request: FullScanRequest<K, I>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error>;

    /// Sync a set of scripts with the blockchain (via an Esplora client) for the data
    /// specified and return a [`TxGraph`].
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`full_scan`]: EsploraExt::full_scan
    fn sync(&self, request: SyncRequest, parallel_requests: usize) -> Result<SyncResult, Error>;
}

impl EsploraExt for esplora_client::BlockingClient {
    fn full_scan<
        K: Ord + Clone + Send + Debug + 'static,
        I: Iterator<Item = (u32, ScriptBuf)> + Send,
    >(
        &self,
        mut request: FullScanRequest<K, I>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error> {
        let update_blocks = init_chain_update_blocking(self, &request.chain_tip)?;
        let (graph_update, last_active_indices) = full_scan_for_index_and_graph_blocking(
            self,
            request.take_spks_by_keychain(),
            stop_gap,
            parallel_requests,
        )?;
        let chain_update = finalize_chain_update_blocking(
            self,
            &request.chain_tip,
            graph_update.all_anchors(),
            update_blocks,
        )?;
        Ok(FullScanResult {
            graph_update,
            chain_update,
            last_active_indices,
        })
    }

    fn sync(
        &self,
        mut request: SyncRequest,
        parallel_requests: usize,
    ) -> Result<SyncResult, Error> {
        let update_blocks = init_chain_update_blocking(self, &request.chain_tip)?;
        let graph_update = sync_for_index_and_graph_blocking(
            self,
            request.take_spks().map(|(_i, spk)| spk),
            request.take_txids(),
            request.take_outpoints(),
            parallel_requests,
        )?;
        let chain_update = finalize_chain_update_blocking(
            self,
            &request.chain_tip,
            graph_update.all_anchors(),
            update_blocks,
        )?;
        Ok(SyncResult {
            graph_update,
            chain_update,
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
#[doc(hidden)]
pub fn init_chain_update_blocking(
    client: &esplora_client::BlockingClient,
    local_tip: &CheckPoint,
) -> Result<BTreeMap<u32, BlockHash>, Error> {
    // Fetch latest N (server dependent) blocks from Esplora. The server guarantees these are
    // consistent.
    let mut fetched_blocks = client
        .get_blocks(None)?
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
            btree_map::Entry::Vacant(entry) => *entry.insert(client.get_block_hash(height)?),
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
#[doc(hidden)]
pub fn finalize_chain_update_blocking<A: Anchor>(
    client: &esplora_client::BlockingClient,
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
        .map(|(a, _)| a.anchor_block().height)
        // filter out duplicate heights
        .filter({
            let mut prev_height = Option::<u32>::None;
            move |h| match prev_height.replace(*h) {
                None => true,
                Some(prev_h) => prev_h != *h,
            }
        })
        // filter out heights that surpass the update tip
        .filter(|h| *h <= update_tip_height)
        .rev();

    // We keep track of a checkpoint node of `local_tip` to make traversing the linked-list of
    // checkpoints more efficient.
    let mut curr_cp = local_tip.clone();

    for h in anchor_heights {
        if let Some(cp) = curr_cp.query_from(h) {
            curr_cp = cp.clone();
            if cp.height() == h {
                // blocks that already exist in checkpoint linked-list is also stored in
                // `update_blocks` because we want to keep higher checkpoints of `local_chain`
                update_blocks.insert(h, cp.hash());
                continue;
            }
        }
        if let btree_map::Entry::Vacant(entry) = update_blocks.entry(h) {
            entry.insert(client.get_block_hash(h)?);
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
#[doc(hidden)]
pub fn full_scan_for_index_and_graph_blocking<K: Ord + Clone + Send>(
    client: &esplora_client::BlockingClient,
    keychain_spks: BTreeMap<
        K,
        Box<impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send>,
    >,
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
                                value: prevout.value,
                            },
                        ))
                    });

                    for (outpoint, txout) in previous_outputs {
                        let _ = tx_graph.insert_txout(outpoint, txout);
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
            last_active_indices.insert(keychain, last_active_index);
        }
    }

    Ok((tx_graph, last_active_indices))
}

#[doc(hidden)]
pub fn sync_for_index_and_graph_blocking(
    client: &esplora_client::BlockingClient,
    misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = ScriptBuf> + Send> + Send,
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
                Box::new(
                    misc_spks
                        .into_iter()
                        .enumerate()
                        .map(|(i, spk)| (i as u32, spk)),
                ),
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
