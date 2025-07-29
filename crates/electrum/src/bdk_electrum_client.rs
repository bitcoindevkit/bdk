use bdk_core::{
    bitcoin::{block::Header, BlockHash, OutPoint, Transaction, Txid},
    collections::{BTreeMap, HashMap, HashSet},
    spk_client::{
        FullScanRequest, FullScanResponse, SpkWithExpectedTxids, SyncRequest, SyncResponse,
    },
    BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate,
};
use electrum_client::{ElectrumApi, Error, HeaderNotification};
use std::sync::{Arc, Mutex};

/// We include a chain suffix of a certain length for the purpose of robustness.
const CHAIN_SUFFIX_LENGTH: u32 = 8;

/// Wrapper around an [`electrum_client::ElectrumApi`] which includes an internal in-memory
/// transaction cache to avoid re-fetching already downloaded transactions.
#[derive(Debug)]
pub struct BdkElectrumClient<E> {
    /// The internal [`electrum_client::ElectrumApi`]
    pub inner: E,
    /// The transaction cache
    tx_cache: Mutex<HashMap<Txid, Arc<Transaction>>>,
    /// The header cache
    block_header_cache: Mutex<HashMap<u32, Header>>,
    /// Cache of transaction anchors
    anchor_cache: Mutex<HashMap<(Txid, BlockHash), ConfirmationBlockTime>>,
}

impl<E: ElectrumApi> BdkElectrumClient<E> {
    /// Creates a new bdk client from a [`electrum_client::ElectrumApi`]
    pub fn new(client: E) -> Self {
        Self {
            inner: client,
            tx_cache: Default::default(),
            block_header_cache: Default::default(),
            anchor_cache: Default::default(),
        }
    }

    /// Inserts transactions into the transaction cache so that the client will not fetch these
    /// transactions.
    pub fn populate_tx_cache(&self, txs: impl IntoIterator<Item = impl Into<Arc<Transaction>>>) {
        let mut tx_cache = self.tx_cache.lock().unwrap();
        for tx in txs {
            let tx = tx.into();
            let txid = tx.compute_txid();
            tx_cache.insert(txid, tx);
        }
    }

    /// Fetch transaction of given `txid`.
    ///
    /// If it hits the cache it will return the cached version and avoid making the request.
    pub fn fetch_tx(&self, txid: Txid) -> Result<Arc<Transaction>, Error> {
        let tx_cache = self.tx_cache.lock().unwrap();

        if let Some(tx) = tx_cache.get(&txid) {
            return Ok(Arc::clone(tx));
        }

        drop(tx_cache);

        let tx = Arc::new(self.inner.transaction_get(&txid)?);

        self.tx_cache.lock().unwrap().insert(txid, Arc::clone(&tx));

        Ok(tx)
    }

    /// Broadcasts a transaction to the network.
    ///
    /// This is a re-export of [`ElectrumApi::transaction_broadcast`].
    pub fn transaction_broadcast(&self, tx: &Transaction) -> Result<Txid, Error> {
        self.inner.transaction_broadcast(tx)
    }

    /// Full scan the keychain scripts specified with the blockchain (via an Electrum client) and
    /// returns updates for [`bdk_chain`] data structures.
    ///
    /// - `request`: struct with data required to perform a spk-based blockchain client full scan,
    ///   see [`FullScanRequest`].
    /// - `stop_gap`: the full scan for each keychain stops after a gap of script pubkeys with no
    ///   associated transactions.
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///   request.
    /// - `fetch_prev_txouts`: specifies whether we want previous `TxOut`s for fee calculation. Note
    ///   that this requires additional calls to the Electrum server, but is necessary for
    ///   calculating the fee on a transaction if your wallet does not own the inputs. Methods like
    ///   [`Wallet.calculate_fee`] and [`Wallet.calculate_fee_rate`] will return a
    ///   [`CalculateFeeError::MissingTxOut`] error if those `TxOut`s are not present in the
    ///   transaction graph.
    ///
    /// [`bdk_chain`]: ../bdk_chain/index.html
    /// [`CalculateFeeError::MissingTxOut`]: ../bdk_chain/tx_graph/enum.CalculateFeeError.html#variant.MissingTxOut
    /// [`Wallet.calculate_fee`]: ../bdk_wallet/struct.Wallet.html#method.calculate_fee
    /// [`Wallet.calculate_fee_rate`]: ../bdk_wallet/struct.Wallet.html#method.calculate_fee_rate
    pub fn full_scan<K: Ord + Clone>(
        &self,
        request: impl Into<FullScanRequest<K>>,
        stop_gap: usize,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<FullScanResponse<K>, Error> {
        let mut request: FullScanRequest<K> = request.into();
        let start_time = request.start_time();

        let tip_and_latest_blocks = match request.chain_tip() {
            Some(chain_tip) => Some(fetch_tip_and_latest_blocks(&self.inner, chain_tip)?),
            None => None,
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        let mut last_active_indices = BTreeMap::<K, u32>::default();
        let mut pending_anchors = Vec::new();
        for keychain in request.keychains() {
            let spks = request
                .iter_spks(keychain.clone())
                .map(|(spk_i, spk)| (spk_i, SpkWithExpectedTxids::from(spk)));
            if let Some(last_active_index) = self.populate_with_spks(
                start_time,
                &mut tx_update,
                spks,
                stop_gap,
                batch_size,
                &mut pending_anchors,
            )? {
                last_active_indices.insert(keychain, last_active_index);
            }
        }

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            self.fetch_prev_txout(&mut tx_update)?;
        }

        if !pending_anchors.is_empty() {
            let anchors = self.batch_fetch_anchors(&pending_anchors)?;
            for (txid, anchor) in anchors {
                tx_update.anchors.insert((anchor, txid));
            }
        }

        let chain_update = match tip_and_latest_blocks {
            Some((chain_tip, latest_blocks)) => Some(chain_update(
                chain_tip,
                &latest_blocks,
                tx_update.anchors.iter().cloned(),
            )?),
            _ => None,
        };

        Ok(FullScanResponse {
            tx_update,
            chain_update,
            last_active_indices,
        })
    }

    /// Sync a set of scripts with the blockchain (via an Electrum client) for the data specified
    /// and returns updates for [`bdk_chain`] data structures.
    ///
    /// - `request`: struct with data required to perform a spk-based blockchain client sync, see
    ///   [`SyncRequest`]
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///   request
    /// - `fetch_prev_txouts`: specifies whether we want previous `TxOut`s for fee calculation. Note
    ///   that this requires additional calls to the Electrum server, but is necessary for
    ///   calculating the fee on a transaction if your wallet does not own the inputs. Methods like
    ///   [`Wallet.calculate_fee`] and [`Wallet.calculate_fee_rate`] will return a
    ///   [`CalculateFeeError::MissingTxOut`] error if those `TxOut`s are not present in the
    ///   transaction graph.
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`full_scan`]: Self::full_scan
    /// [`bdk_chain`]: ../bdk_chain/index.html
    /// [`CalculateFeeError::MissingTxOut`]: ../bdk_chain/tx_graph/enum.CalculateFeeError.html#variant.MissingTxOut
    /// [`Wallet.calculate_fee`]: ../bdk_wallet/struct.Wallet.html#method.calculate_fee
    /// [`Wallet.calculate_fee_rate`]: ../bdk_wallet/struct.Wallet.html#method.calculate_fee_rate
    pub fn sync<I: 'static>(
        &self,
        request: impl Into<SyncRequest<I>>,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<SyncResponse, Error> {
        let mut request: SyncRequest<I> = request.into();
        let start_time = request.start_time();

        let tip_and_latest_blocks = match request.chain_tip() {
            Some(chain_tip) => Some(fetch_tip_and_latest_blocks(&self.inner, chain_tip)?),
            None => None,
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        let mut pending_anchors = Vec::new();
        self.populate_with_spks(
            start_time,
            &mut tx_update,
            request
                .iter_spks_with_expected_txids()
                .enumerate()
                .map(|(i, spk)| (i as u32, spk)),
            usize::MAX,
            batch_size,
            &mut pending_anchors,
        )?;
        self.populate_with_txids(
            start_time,
            &mut tx_update,
            request.iter_txids(),
            &mut pending_anchors,
        )?;
        self.populate_with_outpoints(
            start_time,
            &mut tx_update,
            request.iter_outpoints(),
            &mut pending_anchors,
        )?;

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            self.fetch_prev_txout(&mut tx_update)?;
        }

        if !pending_anchors.is_empty() {
            let anchors = self.batch_fetch_anchors(&pending_anchors)?;
            for (txid, anchor) in anchors {
                tx_update.anchors.insert((anchor, txid));
            }
        }

        let chain_update = match tip_and_latest_blocks {
            Some((chain_tip, latest_blocks)) => Some(chain_update(
                chain_tip,
                &latest_blocks,
                tx_update.anchors.iter().cloned(),
            )?),
            None => None,
        };

        Ok(SyncResponse {
            tx_update,
            chain_update,
        })
    }

    /// Populate the `tx_update` with transactions/anchors associated with the given `spks`.
    ///
    /// Transactions that contains an output with requested spk, or spends form an output with
    /// requested spk will be added to `tx_update`. Anchors of the aforementioned transactions are
    /// also included.
    fn populate_with_spks(
        &self,
        start_time: u64,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        mut spks_with_expected_txids: impl Iterator<Item = (u32, SpkWithExpectedTxids)>,
        stop_gap: usize,
        batch_size: usize,
        pending_anchors: &mut Vec<(Txid, usize)>,
    ) -> Result<Option<u32>, Error> {
        let mut unused_spk_count = 0_usize;
        let mut last_active_index = Option::<u32>::None;

        loop {
            let spks = (0..batch_size)
                .map_while(|_| spks_with_expected_txids.next())
                .collect::<Vec<_>>();
            if spks.is_empty() {
                return Ok(last_active_index);
            }

            let spk_histories = self
                .inner
                .batch_script_get_history(spks.iter().map(|(_, s)| s.spk.as_script()))?;

            for ((spk_index, spk), spk_history) in spks.into_iter().zip(spk_histories) {
                if spk_history.is_empty() {
                    match unused_spk_count.checked_add(1) {
                        Some(i) if i < stop_gap => unused_spk_count = i,
                        _ => return Ok(last_active_index),
                    };
                } else {
                    last_active_index = Some(spk_index);
                    unused_spk_count = 0;
                }

                let spk_history_set = spk_history
                    .iter()
                    .map(|res| res.tx_hash)
                    .collect::<HashSet<_>>();

                tx_update.evicted_ats.extend(
                    spk.expected_txids
                        .difference(&spk_history_set)
                        .map(|&txid| (txid, start_time)),
                );

                for tx_res in spk_history {
                    tx_update.txs.push(self.fetch_tx(tx_res.tx_hash)?);
                    match tx_res.height.try_into() {
                        // Returned heights 0 & -1 are reserved for unconfirmed txs.
                        Ok(height) if height > 0 => {
                            pending_anchors.push((tx_res.tx_hash, height));
                        }
                        _ => {
                            tx_update.seen_ats.insert((tx_res.tx_hash, start_time));
                        }
                    }
                }
            }
        }
    }

    /// Populate the `tx_update` with associated transactions/anchors of `outpoints`.
    ///
    /// Transactions in which the outpoint resides, and transactions that spend from the outpoint
    /// are included. Anchors of the aforementioned transactions are included.
    fn populate_with_outpoints(
        &self,
        start_time: u64,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        pending_anchors: &mut Vec<(Txid, usize)>,
    ) -> Result<(), Error> {
        // Collect valid outpoints with their corresponding `spk` and `tx`.
        let mut ops_spks_txs = Vec::new();
        for op in outpoints {
            if let Ok(tx) = self.fetch_tx(op.txid) {
                if let Some(txout) = tx.output.get(op.vout as usize) {
                    ops_spks_txs.push((op, txout.script_pubkey.clone(), tx));
                }
            }
        }

        // Dedup `spk`s, batch-fetch all histories in one call, and store them in a map.
        let unique_spks: Vec<_> = ops_spks_txs
            .iter()
            .map(|(_, spk, _)| spk.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let histories = self
            .inner
            .batch_script_get_history(unique_spks.iter().map(|spk| spk.as_script()))?;
        let mut spk_map = HashMap::new();
        for (spk, history) in unique_spks.into_iter().zip(histories.into_iter()) {
            spk_map.insert(spk, history);
        }

        for (outpoint, spk, tx) in ops_spks_txs {
            if let Some(spk_history) = spk_map.get(&spk) {
                let mut has_residing = false; // tx in which the outpoint resides
                let mut has_spending = false; // tx that spends the outpoint

                for res in spk_history {
                    if has_residing && has_spending {
                        break;
                    }

                    if !has_residing && res.tx_hash == outpoint.txid {
                        has_residing = true;
                        tx_update.txs.push(Arc::clone(&tx));
                        match res.height.try_into() {
                            // Returned heights 0 & -1 are reserved for unconfirmed txs.
                            Ok(height) if height > 0 => {
                                pending_anchors.push((res.tx_hash, height));
                            }
                            _ => {
                                tx_update.seen_ats.insert((res.tx_hash, start_time));
                            }
                        }
                    }

                    if !has_spending && res.tx_hash != outpoint.txid {
                        let res_tx = self.fetch_tx(res.tx_hash)?;
                        // we exclude txs/anchors that do not spend our specified outpoint(s)
                        has_spending = res_tx
                            .input
                            .iter()
                            .any(|txin| txin.previous_output == outpoint);
                        if !has_spending {
                            continue;
                        }
                        tx_update.txs.push(Arc::clone(&res_tx));
                        match res.height.try_into() {
                            // Returned heights 0 & -1 are reserved for unconfirmed txs.
                            Ok(height) if height > 0 => {
                                pending_anchors.push((res.tx_hash, height));
                            }
                            _ => {
                                tx_update.seen_ats.insert((res.tx_hash, start_time));
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Populate the `tx_update` with transactions/anchors of the provided `txids`.
    fn populate_with_txids(
        &self,
        start_time: u64,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        txids: impl IntoIterator<Item = Txid>,
        pending_anchors: &mut Vec<(Txid, usize)>,
    ) -> Result<(), Error> {
        let mut txs = Vec::<(Txid, Arc<Transaction>)>::new();
        let mut scripts = Vec::new();
        for txid in txids {
            match self.fetch_tx(txid) {
                Ok(tx) => {
                    let spk = tx
                        .output
                        .first()
                        .map(|txo| &txo.script_pubkey)
                        .expect("tx must have an output")
                        .clone();
                    txs.push((txid, tx));
                    scripts.push(spk);
                }
                Err(electrum_client::Error::Protocol(_)) => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        // because of restrictions of the Electrum API, we have to use the `script_get_history`
        // call to get confirmation status of our transaction
        let spk_histories = self
            .inner
            .batch_script_get_history(scripts.iter().map(|spk| spk.as_script()))?;

        for (tx, spk_history) in txs.into_iter().zip(spk_histories) {
            if let Some(res) = spk_history.into_iter().find(|res| res.tx_hash == tx.0) {
                match res.height.try_into() {
                    // Returned heights 0 & -1 are reserved for unconfirmed txs.
                    Ok(height) if height > 0 => {
                        pending_anchors.push((tx.0, height));
                    }
                    _ => {
                        tx_update.seen_ats.insert((res.tx_hash, start_time));
                    }
                }
            }

            tx_update.txs.push(tx.1);
        }

        Ok(())
    }

    /// Batch validate Merkle proofs, cache each confirmation anchor, and return them.
    fn batch_fetch_anchors(
        &self,
        txs_with_heights: &[(Txid, usize)],
    ) -> Result<Vec<(Txid, ConfirmationBlockTime)>, Error> {
        let mut results = Vec::with_capacity(txs_with_heights.len());
        let mut to_fetch = Vec::new();

        // Figure out which block heights we need headers for.
        let mut needed_heights: Vec<u32> =
            txs_with_heights.iter().map(|&(_, h)| h as u32).collect();
        needed_heights.sort_unstable();
        needed_heights.dedup();

        let mut height_to_hash = HashMap::with_capacity(needed_heights.len());

        // Collect headers of missing heights, and build `height_to_hash` map.
        {
            let mut cache = self.block_header_cache.lock().unwrap();

            let mut missing_heights = Vec::new();
            for &height in &needed_heights {
                if let Some(header) = cache.get(&height) {
                    height_to_hash.insert(height, header.block_hash());
                } else {
                    missing_heights.push(height);
                }
            }

            if !missing_heights.is_empty() {
                let headers = self.inner.batch_block_header(missing_heights.clone())?;
                for (height, header) in missing_heights.into_iter().zip(headers) {
                    height_to_hash.insert(height, header.block_hash());
                    cache.insert(height, header);
                }
            }
        }

        // Check our anchor cache and queue up any proofs we still need.
        {
            let anchor_cache = self.anchor_cache.lock().unwrap();
            for &(txid, height) in txs_with_heights {
                let h = height as u32;
                let hash = height_to_hash[&h];
                if let Some(anchor) = anchor_cache.get(&(txid, hash)) {
                    results.push((txid, *anchor));
                } else {
                    to_fetch.push((txid, height, hash));
                }
            }
        }

        // Batch all get_merkle calls.
        let mut batch = electrum_client::Batch::default();
        for &(txid, height, _) in &to_fetch {
            batch.raw(
                "blockchain.transaction.get_merkle".into(),
                vec![
                    electrum_client::Param::String(format!("{txid:x}")),
                    electrum_client::Param::Usize(height),
                ],
            );
        }
        let resps = self.inner.batch_call(&batch)?;

        // Validate each proof, retrying once for each stale header.
        for ((txid, height, hash), resp) in to_fetch.into_iter().zip(resps.into_iter()) {
            let proof: electrum_client::GetMerkleRes = serde_json::from_value(resp)?;

            let mut header = {
                let cache = self.block_header_cache.lock().unwrap();
                cache
                    .get(&(height as u32))
                    .copied()
                    .expect("header already fetched above")
            };
            let mut valid =
                electrum_client::utils::validate_merkle_proof(&txid, &header.merkle_root, &proof);
            if !valid {
                header = self.inner.block_header(height)?;
                self.block_header_cache
                    .lock()
                    .unwrap()
                    .insert(height as u32, header);
                valid = electrum_client::utils::validate_merkle_proof(
                    &txid,
                    &header.merkle_root,
                    &proof,
                );
            }

            // Build and cache the anchor if merkle proof is valid.
            if valid {
                let anchor = ConfirmationBlockTime {
                    confirmation_time: header.time as u64,
                    block_id: BlockId {
                        height: height as u32,
                        hash,
                    },
                };
                self.anchor_cache
                    .lock()
                    .unwrap()
                    .insert((txid, hash), anchor);
                results.push((txid, anchor));
            }
        }

        Ok(results)
    }

    // Helper function which fetches the `TxOut`s of our relevant transactions' previous
    // transactions, which we do not have by default. This data is needed to calculate the
    // transaction fee.
    fn fetch_prev_txout(
        &self,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) -> Result<(), Error> {
        let mut no_dup = HashSet::<Txid>::new();
        for tx in &tx_update.txs {
            if !tx.is_coinbase() && no_dup.insert(tx.compute_txid()) {
                for vin in &tx.input {
                    let outpoint = vin.previous_output;
                    let vout = outpoint.vout;
                    let prev_tx = self.fetch_tx(outpoint.txid)?;
                    // Ensure server returns the expected txout.
                    let txout = prev_tx
                        .output
                        .get(vout as usize)
                        .ok_or_else(|| {
                            electrum_client::Error::Message(format!(
                                "prevout {outpoint} does not exist"
                            ))
                        })?
                        .clone();
                    tx_update.txouts.insert(outpoint, txout);
                }
            }
        }
        Ok(())
    }
}

/// Return a [`CheckPoint`] of the latest tip, that connects with `prev_tip`. The latest blocks are
/// fetched to construct checkpoint updates with the proper [`BlockHash`] in case of re-org.
fn fetch_tip_and_latest_blocks(
    client: &impl ElectrumApi,
    prev_tip: CheckPoint,
) -> Result<(CheckPoint, BTreeMap<u32, BlockHash>), Error> {
    let HeaderNotification { height, .. } = client.block_headers_subscribe()?;
    let new_tip_height = height as u32;

    // If electrum returns a tip height that is lower than our previous tip, then checkpoints do
    // not need updating. We just return the previous tip and use that as the point of agreement.
    if new_tip_height < prev_tip.height() {
        return Ok((prev_tip, BTreeMap::new()));
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
            .ok_or_else(|| Error::Message("cannot find agreement block with server".to_string()))?
    };

    let extension = new_blocks
        .iter()
        .filter({
            let agreement_height = agreement_cp.height();
            move |(height, _)| **height > agreement_height
        })
        .map(|(&height, &hash)| BlockId { height, hash });
    let new_tip = agreement_cp
        .extend(extension)
        .expect("extension heights already checked to be greater than agreement height");

    Ok((new_tip, new_blocks))
}

// Add a corresponding checkpoint per anchor height if it does not yet exist. Checkpoints should not
// surpass `latest_blocks`.
fn chain_update(
    mut tip: CheckPoint,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    anchors: impl Iterator<Item = (ConfirmationBlockTime, Txid)>,
) -> Result<CheckPoint, Error> {
    for (anchor, _txid) in anchors {
        let height = anchor.block_id.height;

        // Checkpoint uses the `BlockHash` from `latest_blocks` so that the hash will be consistent
        // in case of a re-org.
        if tip.get(height).is_none() && height <= tip.height() {
            let hash = match latest_blocks.get(&height) {
                Some(&hash) => hash,
                None => anchor.block_id.hash,
            };
            tip = tip.insert(BlockId { hash, height });
        }
    }
    Ok(tip)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use crate::{bdk_electrum_client::TxUpdate, BdkElectrumClient};
    use bdk_chain::bitcoin::{constants, Network, OutPoint, ScriptBuf, Transaction, TxIn};
    use bdk_chain::{BlockId, CheckPoint};
    use bdk_core::{collections::BTreeMap, spk_client::SyncRequest};
    use bdk_testenv::{anyhow, utils::new_tx, TestEnv};
    use electrum_client::Error as ElectrumError;
    use std::sync::Arc;

    #[cfg(feature = "default")]
    #[test]
    fn test_fetch_prev_txout_with_coinbase() {
        let env = TestEnv::new().unwrap();
        let electrum_client =
            electrum_client::Client::new(env.electrsd.electrum_url.as_str()).unwrap();
        let client = BdkElectrumClient::new(electrum_client);

        // Create a coinbase transaction.
        let coinbase_tx = Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                ..Default::default()
            }],
            ..new_tx(0)
        };

        assert!(coinbase_tx.is_coinbase());

        // Test that `fetch_prev_txout` does not process coinbase transactions. Calling
        // `fetch_prev_txout` on a coinbase transaction will trigger a `fetch_tx` on a transaction
        // with a txid of all zeros. If `fetch_prev_txout` attempts to fetch this transaction, this
        // assertion will fail.
        let mut tx_update = TxUpdate::default();
        tx_update.txs = vec![Arc::new(coinbase_tx)];
        assert!(client.fetch_prev_txout(&mut tx_update).is_ok());

        // Ensure that the txouts are empty.
        assert_eq!(tx_update.txouts, BTreeMap::default());
    }

    #[cfg(feature = "default")]
    #[test]
    fn test_sync_wrong_network_error() -> anyhow::Result<()> {
        let env = TestEnv::new()?;
        let client = electrum_client::Client::new(env.electrsd.electrum_url.as_str()).unwrap();
        let electrum_client = BdkElectrumClient::new(client);

        let _ = env.mine_blocks(1, None).unwrap();

        let bogus_spks: Vec<ScriptBuf> = Vec::new();
        let bogus_genesis = constants::genesis_block(Network::Testnet).block_hash();
        let bogus_cp = CheckPoint::new(BlockId {
            height: 0,
            hash: bogus_genesis,
        });

        let req = SyncRequest::builder()
            .chain_tip(bogus_cp)
            .spks(bogus_spks)
            .build();
        let err = electrum_client.sync(req, 1, false).unwrap_err();

        assert!(
            matches!(err, ElectrumError::Message(m) if m.contains("cannot find agreement block with server")),
            "expected missing agreement block error"
        );

        Ok(())
    }
}
