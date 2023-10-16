use std::thread::JoinHandle;

use bdk_chain::collections::btree_map;
use bdk_chain::collections::BTreeMap;
use bdk_chain::{
    bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf, TxOut, Txid},
    local_chain::{self, CheckPoint},
    BlockId, ConfirmationTimeHeightAnchor, TxGraph,
};
use esplora_client::TxStatus;

use crate::anchor_from_status;

/// [`esplora_client::Error`]
type Error = Box<esplora_client::Error>;

/// Trait to extend the functionality of [`esplora_client::BlockingClient`].
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
pub trait EsploraExt {
    /// Prepare a [`LocalChain`] update with blocks fetched from Esplora.
    ///
    /// * `local_tip` is the previous tip of [`LocalChain::tip`].
    /// * `request_heights` is the block heights that we are interested in fetching from Esplora.
    ///
    /// The result of this method can be applied to [`LocalChain::apply_update`].
    ///
    /// ## Consistency
    ///
    /// The chain update returned is guaranteed to be consistent as long as there is not a *large* re-org
    /// during the call. The size of re-org we can tollerate is server dependent but will be at
    /// least 10.
    ///
    /// [`LocalChain`]: bdk_chain::local_chain::LocalChain
    /// [`LocalChain::tip`]: bdk_chain::local_chain::LocalChain::tip
    /// [`LocalChain::apply_update`]: bdk_chain::local_chain::LocalChain::apply_update
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
        // Fetch latest N (server dependent) blocks from Esplora. The server guarantees these are
        // consistent.
        let mut fetched_blocks = self
            .get_blocks(None)?
            .into_iter()
            .map(|b| (b.time.height, b.id))
            .collect::<BTreeMap<u32, BlockHash>>();
        let new_tip_height = fetched_blocks
            .keys()
            .last()
            .copied()
            .expect("must atleast have one block");

        // Fetch blocks of heights that the caller is interested in, skipping blocks that are
        // already fetched when constructing `fetched_blocks`.
        for height in request_heights {
            // do not fetch blocks higher than remote tip
            if height > new_tip_height {
                continue;
            }
            // only fetch what is missing
            if let btree_map::Entry::Vacant(entry) = fetched_blocks.entry(height) {
                // ❗The return value of `get_block_hash` is not strictly guaranteed to be consistent
                // with the chain at the time of `get_blocks` above (there could have been a deep
                // re-org). Since `get_blocks` returns 10 (or so) blocks we are assuming that it's
                // not possible to have a re-org deeper than that.
                entry.insert(self.get_block_hash(height)?);
            }
        }

        // Ensure `fetched_blocks` can create an update that connects with the original chain by
        // finding a "Point of Agreement".
        for (height, local_hash) in local_tip.iter().map(|cp| (cp.height(), cp.hash())) {
            if height > new_tip_height {
                continue;
            }

            let fetched_hash = match fetched_blocks.entry(height) {
                btree_map::Entry::Occupied(entry) => *entry.get(),
                btree_map::Entry::Vacant(entry) => *entry.insert(self.get_block_hash(height)?),
            };

            // We have found point of agreement so the update will connect!
            if fetched_hash == local_hash {
                break;
            }
        }

        Ok(local_chain::Update {
            tip: CheckPoint::from_block_ids(fetched_blocks.into_iter().map(BlockId::from))
                .expect("must be in height order"),
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
