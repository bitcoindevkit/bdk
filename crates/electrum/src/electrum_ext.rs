use bdk_chain::{
    bitcoin::{OutPoint, ScriptBuf, Transaction, Txid},
    collections::{BTreeMap, HashMap, HashSet},
    local_chain::CheckPoint,
    spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult, TxCache},
    tx_graph::TxGraph,
    BlockId, ConfirmationHeightAnchor, ConfirmationTimeHeightAnchor,
};
use core::str::FromStr;
use electrum_client::{ElectrumApi, Error, HeaderNotification};
use std::sync::Arc;

/// We include a chain suffix of a certain length for the purpose of robustness.
const CHAIN_SUFFIX_LENGTH: u32 = 8;

/// Trait to extend [`electrum_client::Client`] functionality.
pub trait ElectrumExt {
    /// Full scan the keychain scripts specified with the blockchain (via an Electrum client) and
    /// returns updates for [`bdk_chain`] data structures.
    ///
    /// - `request`: struct with data required to perform a spk-based blockchain client full scan,
    ///              see [`FullScanRequest`]
    /// - `stop_gap`: the full scan for each keychain stops after a gap of script pubkeys with no
    ///              associated transactions
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///              request
    /// - `fetch_prev_txouts`: specifies whether or not we want previous `TxOut`s for fee
    ///              calculation
    fn full_scan<K: Ord + Clone>(
        &self,
        request: FullScanRequest<K>,
        stop_gap: usize,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<ElectrumFullScanResult<K>, Error>;

    /// Sync a set of scripts with the blockchain (via an Electrum client) for the data specified
    /// and returns updates for [`bdk_chain`] data structures.
    ///
    /// - `request`: struct with data required to perform a spk-based blockchain client sync,
    ///              see [`SyncRequest`]
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///              request
    /// - `fetch_prev_txouts`: specifies whether or not we want previous `TxOut`s for fee
    ///              calculation
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`full_scan`]: ElectrumExt::full_scan
    fn sync(
        &self,
        request: SyncRequest,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<ElectrumSyncResult, Error>;
}

impl<E: ElectrumApi> ElectrumExt for E {
    fn full_scan<K: Ord + Clone>(
        &self,
        mut request: FullScanRequest<K>,
        stop_gap: usize,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<ElectrumFullScanResult<K>, Error> {
        let mut request_spks = request.spks_by_keychain;

        // We keep track of already-scanned spks just in case a reorg happens and we need to do a
        // rescan. We need to keep track of this as iterators in `keychain_spks` are "unbounded" so
        // cannot be collected. In addition, we keep track of whether an spk has an active tx
        // history for determining the `last_active_index`.
        // * key: (keychain, spk_index) that identifies the spk.
        // * val: (script_pubkey, has_tx_history).
        let mut scanned_spks = BTreeMap::<(K, u32), (ScriptBuf, bool)>::new();

        let update = loop {
            let (tip, _) = construct_update_tip(self, request.chain_tip.clone())?;
            let mut graph_update = TxGraph::<ConfirmationHeightAnchor>::default();
            let cps = tip
                .iter()
                .take(10)
                .map(|cp| (cp.height(), cp))
                .collect::<BTreeMap<u32, CheckPoint>>();

            if !request_spks.is_empty() {
                if !scanned_spks.is_empty() {
                    scanned_spks.append(&mut populate_with_spks(
                        self,
                        &cps,
                        &mut request.tx_cache,
                        &mut graph_update,
                        &mut scanned_spks
                            .iter()
                            .map(|(i, (spk, _))| (i.clone(), spk.clone())),
                        stop_gap,
                        batch_size,
                    )?);
                }
                for (keychain, keychain_spks) in &mut request_spks {
                    scanned_spks.extend(
                        populate_with_spks(
                            self,
                            &cps,
                            &mut request.tx_cache,
                            &mut graph_update,
                            keychain_spks,
                            stop_gap,
                            batch_size,
                        )?
                        .into_iter()
                        .map(|(spk_i, spk)| ((keychain.clone(), spk_i), spk)),
                    );
                }
            }

            // check for reorgs during scan process
            let server_blockhash = self.block_header(tip.height() as usize)?.block_hash();
            if tip.hash() != server_blockhash {
                continue; // reorg
            }

            // Fetch previous `TxOut`s for fee calculation if flag is enabled.
            if fetch_prev_txouts {
                fetch_prev_txout(self, &mut request.tx_cache, &mut graph_update)?;
            }

            let chain_update = tip;

            let keychain_update = request_spks
                .into_keys()
                .filter_map(|k| {
                    scanned_spks
                        .range((k.clone(), u32::MIN)..=(k.clone(), u32::MAX))
                        .rev()
                        .find(|(_, (_, active))| *active)
                        .map(|((_, i), _)| (k, *i))
                })
                .collect::<BTreeMap<_, _>>();

            break FullScanResult {
                graph_update,
                chain_update,
                last_active_indices: keychain_update,
            };
        };

        Ok(ElectrumFullScanResult(update))
    }

    fn sync(
        &self,
        request: SyncRequest,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<ElectrumSyncResult, Error> {
        let mut tx_cache = request.tx_cache.clone();

        let full_scan_req = FullScanRequest::from_chain_tip(request.chain_tip.clone())
            .cache_txs(request.tx_cache)
            .set_spks_for_keychain((), request.spks.enumerate().map(|(i, spk)| (i as u32, spk)));
        let mut full_scan_res = self
            .full_scan(full_scan_req, usize::MAX, batch_size, false)?
            .with_confirmation_height_anchor();

        let (tip, _) = construct_update_tip(self, request.chain_tip)?;
        let cps = tip
            .iter()
            .take(10)
            .map(|cp| (cp.height(), cp))
            .collect::<BTreeMap<u32, CheckPoint>>();

        populate_with_txids(
            self,
            &cps,
            &mut tx_cache,
            &mut full_scan_res.graph_update,
            request.txids,
        )?;
        populate_with_outpoints(
            self,
            &cps,
            &mut tx_cache,
            &mut full_scan_res.graph_update,
            request.outpoints,
        )?;

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            fetch_prev_txout(self, &mut tx_cache, &mut full_scan_res.graph_update)?;
        }

        Ok(ElectrumSyncResult(SyncResult {
            chain_update: full_scan_res.chain_update,
            graph_update: full_scan_res.graph_update,
        }))
    }
}

/// The result of [`ElectrumExt::full_scan`].
///
/// This can be transformed into a [`FullScanResult`] with either [`ConfirmationHeightAnchor`] or
/// [`ConfirmationTimeHeightAnchor`] anchor types.
pub struct ElectrumFullScanResult<K>(FullScanResult<K, ConfirmationHeightAnchor>);

impl<K> ElectrumFullScanResult<K> {
    /// Return [`FullScanResult`] with [`ConfirmationHeightAnchor`].
    pub fn with_confirmation_height_anchor(self) -> FullScanResult<K, ConfirmationHeightAnchor> {
        self.0
    }

    /// Return [`FullScanResult`] with [`ConfirmationTimeHeightAnchor`].
    ///
    /// This requires additional calls to the Electrum server.
    pub fn with_confirmation_time_height_anchor(
        self,
        client: &impl ElectrumApi,
    ) -> Result<FullScanResult<K, ConfirmationTimeHeightAnchor>, Error> {
        let res = self.0;
        Ok(FullScanResult {
            graph_update: try_into_confirmation_time_result(res.graph_update, client)?,
            chain_update: res.chain_update,
            last_active_indices: res.last_active_indices,
        })
    }
}

/// The result of [`ElectrumExt::sync`].
///
/// This can be transformed into a [`SyncResult`] with either [`ConfirmationHeightAnchor`] or
/// [`ConfirmationTimeHeightAnchor`] anchor types.
pub struct ElectrumSyncResult(SyncResult<ConfirmationHeightAnchor>);

impl ElectrumSyncResult {
    /// Return [`SyncResult`] with [`ConfirmationHeightAnchor`].
    pub fn with_confirmation_height_anchor(self) -> SyncResult<ConfirmationHeightAnchor> {
        self.0
    }

    /// Return [`SyncResult`] with [`ConfirmationTimeHeightAnchor`].
    ///
    /// This requires additional calls to the Electrum server.
    pub fn with_confirmation_time_height_anchor(
        self,
        client: &impl ElectrumApi,
    ) -> Result<SyncResult<ConfirmationTimeHeightAnchor>, Error> {
        let res = self.0;
        Ok(SyncResult {
            graph_update: try_into_confirmation_time_result(res.graph_update, client)?,
            chain_update: res.chain_update,
        })
    }
}

fn try_into_confirmation_time_result(
    graph_update: TxGraph<ConfirmationHeightAnchor>,
    client: &impl ElectrumApi,
) -> Result<TxGraph<ConfirmationTimeHeightAnchor>, Error> {
    let relevant_heights = graph_update
        .all_anchors()
        .iter()
        .map(|(a, _)| a.confirmation_height)
        .collect::<HashSet<_>>();

    let height_to_time = relevant_heights
        .clone()
        .into_iter()
        .zip(
            client
                .batch_block_header(relevant_heights)?
                .into_iter()
                .map(|bh| bh.time as u64),
        )
        .collect::<HashMap<u32, u64>>();

    Ok(graph_update.map_anchors(|a| ConfirmationTimeHeightAnchor {
        anchor_block: a.anchor_block,
        confirmation_height: a.confirmation_height,
        confirmation_time: height_to_time[&a.confirmation_height],
    }))
}

/// Return a [`CheckPoint`] of the latest tip, that connects with `prev_tip`.
fn construct_update_tip(
    client: &impl ElectrumApi,
    prev_tip: CheckPoint,
) -> Result<(CheckPoint, Option<u32>), Error> {
    let HeaderNotification { height, .. } = client.block_headers_subscribe()?;
    let new_tip_height = height as u32;

    // If electrum returns a tip height that is lower than our previous tip, then checkpoints do
    // not need updating. We just return the previous tip and use that as the point of agreement.
    if new_tip_height < prev_tip.height() {
        return Ok((prev_tip.clone(), Some(prev_tip.height())));
    }

    // Atomically fetch the latest `CHAIN_SUFFIX_LENGTH` count of blocks from Electrum. We use this
    // to construct our checkpoint update.
    let mut new_blocks = {
        let start_height = new_tip_height.saturating_sub(CHAIN_SUFFIX_LENGTH - 1);
        let hashes = client
            .block_headers(start_height as _, CHAIN_SUFFIX_LENGTH as _)?
            .headers
            .into_iter()
            .map(|h| h.block_hash());
        (start_height..).zip(hashes).collect::<BTreeMap<u32, _>>()
    };

    // Find the "point of agreement" (if any).
    let agreement_cp = {
        let mut agreement_cp = Option::<CheckPoint>::None;
        for cp in prev_tip.iter() {
            let cp_block = cp.block_id();
            let hash = match new_blocks.get(&cp_block.height) {
                Some(&hash) => hash,
                None => {
                    assert!(
                        new_tip_height >= cp_block.height,
                        "already checked that electrum's tip cannot be smaller"
                    );
                    let hash = client.block_header(cp_block.height as _)?.block_hash();
                    new_blocks.insert(cp_block.height, hash);
                    hash
                }
            };
            if hash == cp_block.hash {
                agreement_cp = Some(cp);
                break;
            }
        }
        agreement_cp
    };

    let agreement_height = agreement_cp.as_ref().map(CheckPoint::height);

    let new_tip = new_blocks
        .into_iter()
        // Prune `new_blocks` to only include blocks that are actually new.
        .filter(|(height, _)| Some(*height) > agreement_height)
        .map(|(height, hash)| BlockId { height, hash })
        .fold(agreement_cp, |prev_cp, block| {
            Some(match prev_cp {
                Some(cp) => cp.push(block).expect("must extend checkpoint"),
                None => CheckPoint::new(block),
            })
        })
        .expect("must have at least one checkpoint");

    Ok((new_tip, agreement_height))
}

/// A [tx status] comprises of a concatenation of `tx_hash:height:`s. We transform a single one of
/// these concatenations into a [`ConfirmationHeightAnchor`] if possible.
///
/// We use the lowest possible checkpoint as the anchor block (from `cps`). If an anchor block
/// cannot be found, or the transaction is unconfirmed, [`None`] is returned.
///
/// [tx status](https://electrumx-spesmilo.readthedocs.io/en/latest/protocol-basics.html#status)
fn determine_tx_anchor(
    cps: &BTreeMap<u32, CheckPoint>,
    raw_height: i32,
    txid: Txid,
) -> Option<ConfirmationHeightAnchor> {
    // The electrum API has a weird quirk where an unconfirmed transaction is presented with a
    // height of 0. To avoid invalid representation in our data structures, we manually set
    // transactions residing in the genesis block to have height 0, then interpret a height of 0 as
    // unconfirmed for all other transactions.
    if txid
        == Txid::from_str("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
            .expect("must deserialize genesis coinbase txid")
    {
        let anchor_block = cps.values().next()?.block_id();
        return Some(ConfirmationHeightAnchor {
            anchor_block,
            confirmation_height: 0,
        });
    }
    match raw_height {
        h if h <= 0 => {
            debug_assert!(h == 0 || h == -1, "unexpected height ({}) from electrum", h);
            None
        }
        h => {
            let h = h as u32;
            let anchor_block = cps.range(h..).next().map(|(_, cp)| cp.block_id())?;
            if h > anchor_block.height {
                None
            } else {
                Some(ConfirmationHeightAnchor {
                    anchor_block,
                    confirmation_height: h,
                })
            }
        }
    }
}

/// Populate the `graph_update` with associated transactions/anchors of `outpoints`.
///
/// Transactions in which the outpoint resides, and transactions that spend from the outpoint are
/// included. Anchors of the aforementioned transactions are included.
///
/// Checkpoints (in `cps`) are used to create anchors. The `tx_cache` is self-explanatory.
fn populate_with_outpoints(
    client: &impl ElectrumApi,
    cps: &BTreeMap<u32, CheckPoint>,
    tx_cache: &mut TxCache,
    graph_update: &mut TxGraph<ConfirmationHeightAnchor>,
    outpoints: impl IntoIterator<Item = OutPoint>,
) -> Result<(), Error> {
    for outpoint in outpoints {
        let op_txid = outpoint.txid;
        let op_tx = fetch_tx(client, tx_cache, op_txid)?;
        let op_txout = match op_tx.output.get(outpoint.vout as usize) {
            Some(txout) => txout,
            None => continue,
        };
        debug_assert_eq!(op_tx.txid(), op_txid);

        // attempt to find the following transactions (alongside their chain positions), and
        // add to our sparsechain `update`:
        let mut has_residing = false; // tx in which the outpoint resides
        let mut has_spending = false; // tx that spends the outpoint
        for res in client.script_get_history(&op_txout.script_pubkey)? {
            if has_residing && has_spending {
                break;
            }

            if !has_residing && res.tx_hash == op_txid {
                has_residing = true;
                let _ = graph_update.insert_tx(Arc::clone(&op_tx));
                if let Some(anchor) = determine_tx_anchor(cps, res.height, res.tx_hash) {
                    let _ = graph_update.insert_anchor(res.tx_hash, anchor);
                }
            }

            if !has_spending && res.tx_hash != op_txid {
                let res_tx = fetch_tx(client, tx_cache, res.tx_hash)?;
                // we exclude txs/anchors that do not spend our specified outpoint(s)
                has_spending = res_tx
                    .input
                    .iter()
                    .any(|txin| txin.previous_output == outpoint);
                if !has_spending {
                    continue;
                }
                let _ = graph_update.insert_tx(Arc::clone(&res_tx));
                if let Some(anchor) = determine_tx_anchor(cps, res.height, res.tx_hash) {
                    let _ = graph_update.insert_anchor(res.tx_hash, anchor);
                }
            }
        }
    }
    Ok(())
}

/// Populate the `graph_update` with transactions/anchors of the provided `txids`.
fn populate_with_txids(
    client: &impl ElectrumApi,
    cps: &BTreeMap<u32, CheckPoint>,
    tx_cache: &mut TxCache,
    graph_update: &mut TxGraph<ConfirmationHeightAnchor>,
    txids: impl IntoIterator<Item = Txid>,
) -> Result<(), Error> {
    for txid in txids {
        let tx = match fetch_tx(client, tx_cache, txid) {
            Ok(tx) => tx,
            Err(electrum_client::Error::Protocol(_)) => continue,
            Err(other_err) => return Err(other_err),
        };

        let spk = tx
            .output
            .first()
            .map(|txo| &txo.script_pubkey)
            .expect("tx must have an output");

        // because of restrictions of the Electrum API, we have to use the `script_get_history`
        // call to get confirmation status of our transaction
        let anchor = match client
            .script_get_history(spk)?
            .into_iter()
            .find(|r| r.tx_hash == txid)
        {
            Some(r) => determine_tx_anchor(cps, r.height, txid),
            None => continue,
        };

        let _ = graph_update.insert_tx(tx);
        if let Some(anchor) = anchor {
            let _ = graph_update.insert_anchor(txid, anchor);
        }
    }
    Ok(())
}

/// Fetch transaction of given `txid`.
///
/// We maintain a `tx_cache` so that we won't need to fetch from Electrum with every call.
fn fetch_tx<C: ElectrumApi>(
    client: &C,
    tx_cache: &mut TxCache,
    txid: Txid,
) -> Result<Arc<Transaction>, Error> {
    use bdk_chain::collections::hash_map::Entry;
    Ok(match tx_cache.entry(txid) {
        Entry::Occupied(entry) => entry.get().clone(),
        Entry::Vacant(entry) => entry
            .insert(Arc::new(client.transaction_get(&txid)?))
            .clone(),
    })
}

// Helper function which fetches the `TxOut`s of our relevant transactions' previous transactions,
// which we do not have by default. This data is needed to calculate the transaction fee.
fn fetch_prev_txout<C: ElectrumApi>(
    client: &C,
    tx_cache: &mut TxCache,
    graph_update: &mut TxGraph<ConfirmationHeightAnchor>,
) -> Result<(), Error> {
    let full_txs: Vec<Arc<Transaction>> =
        graph_update.full_txs().map(|tx_node| tx_node.tx).collect();
    for tx in full_txs {
        for vin in &tx.input {
            let outpoint = vin.previous_output;
            let prev_tx = fetch_tx(client, tx_cache, outpoint.txid)?;
            for txout in prev_tx.output.clone() {
                let _ = graph_update.insert_txout(outpoint, txout);
            }
        }
    }
    Ok(())
}

/// Populate the `graph_update` with transactions/anchors associated with the given `spks`.
///
/// Transactions that contains an output with requested spk, or spends form an output with
/// requested spk will be added to `graph_update`. Anchors of the aforementioned transactions are
/// also included.
///
/// Checkpoints (in `cps`) are used to create anchors. The `tx_cache` is self-explanatory.
fn populate_with_spks<I: Ord + Clone>(
    client: &impl ElectrumApi,
    cps: &BTreeMap<u32, CheckPoint>,
    tx_cache: &mut TxCache,
    graph_update: &mut TxGraph<ConfirmationHeightAnchor>,
    spks: &mut impl Iterator<Item = (I, ScriptBuf)>,
    stop_gap: usize,
    batch_size: usize,
) -> Result<BTreeMap<I, (ScriptBuf, bool)>, Error> {
    let mut unused_spk_count = 0_usize;
    let mut scanned_spks = BTreeMap::new();

    loop {
        let spks = (0..batch_size)
            .map_while(|_| spks.next())
            .collect::<Vec<_>>();
        if spks.is_empty() {
            return Ok(scanned_spks);
        }

        let spk_histories =
            client.batch_script_get_history(spks.iter().map(|(_, s)| s.as_script()))?;

        for ((spk_index, spk), spk_history) in spks.into_iter().zip(spk_histories) {
            if spk_history.is_empty() {
                scanned_spks.insert(spk_index, (spk, false));
                unused_spk_count += 1;
                if unused_spk_count > stop_gap {
                    return Ok(scanned_spks);
                }
                continue;
            } else {
                scanned_spks.insert(spk_index, (spk, true));
                unused_spk_count = 0;
            }

            for tx_res in spk_history {
                let _ = graph_update.insert_tx(fetch_tx(client, tx_cache, tx_res.tx_hash)?);
                if let Some(anchor) = determine_tx_anchor(cps, tx_res.height, tx_res.tx_hash) {
                    let _ = graph_update.insert_anchor(tx_res.tx_hash, anchor);
                }
            }
        }
    }
}
