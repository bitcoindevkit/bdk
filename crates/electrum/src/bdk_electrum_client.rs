use bdk_chain::{
    bitcoin::{block::Header, BlockHash, OutPoint, ScriptBuf, Transaction, Txid},
    collections::{BTreeMap, HashMap},
    local_chain::CheckPoint,
    spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult},
    tx_graph::TxGraph,
    Anchor, BlockId, BlockTime,
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
}

impl<E: ElectrumApi> BdkElectrumClient<E> {
    /// Creates a new bdk client from a [`electrum_client::ElectrumApi`]
    pub fn new(client: E) -> Self {
        Self {
            inner: client,
            tx_cache: Default::default(),
            block_header_cache: Default::default(),
        }
    }

    /// Inserts transactions into the transaction cache so that the client will not fetch these
    /// transactions.
    pub fn populate_tx_cache<A>(&self, tx_graph: impl AsRef<TxGraph<A>>) {
        let txs = tx_graph
            .as_ref()
            .full_txs()
            .map(|tx_node| (tx_node.txid, tx_node.tx));

        let mut tx_cache = self.tx_cache.lock().unwrap();
        for (txid, tx) in txs {
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

    /// Fetch block header of given `height`.
    ///
    /// If it hits the cache it will return the cached version and avoid making the request.
    fn fetch_header(&self, height: u32) -> Result<Header, Error> {
        let block_header_cache = self.block_header_cache.lock().unwrap();

        if let Some(header) = block_header_cache.get(&height) {
            return Ok(*header);
        }

        drop(block_header_cache);

        self.update_header(height)
    }

    /// Update a block header at given `height`. Returns the updated header.
    fn update_header(&self, height: u32) -> Result<Header, Error> {
        let header = self.inner.block_header(height as usize)?;

        self.block_header_cache
            .lock()
            .unwrap()
            .insert(height, header);

        Ok(header)
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
    ///              see [`FullScanRequest`]
    /// - `stop_gap`: the full scan for each keychain stops after a gap of script pubkeys with no
    ///              associated transactions
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///              request
    /// - `fetch_prev_txouts`: specifies whether or not we want previous `TxOut`s for fee
    pub fn full_scan<K: Ord + Clone>(
        &self,
        request: FullScanRequest<K>,
        stop_gap: usize,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<FullScanResult<K>, Error> {
        let (tip, latest_blocks) =
            fetch_tip_and_latest_blocks(&self.inner, request.chain_tip.clone())?;
        let mut graph_update = TxGraph::<BlockTime>::default();
        let mut last_active_indices = BTreeMap::<K, u32>::new();

        for (keychain, spks) in request.spks_by_keychain {
            if let Some(last_active_index) =
                self.populate_with_spks(&mut graph_update, spks, stop_gap, batch_size)?
            {
                last_active_indices.insert(keychain, last_active_index);
            }
        }

        let chain_update = chain_update(tip, &latest_blocks, graph_update.all_anchors())?;

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            self.fetch_prev_txout(&mut graph_update)?;
        }

        Ok(FullScanResult {
            graph_update,
            chain_update,
            last_active_indices,
        })
    }

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
    /// [`full_scan`]: Self::full_scan
    pub fn sync(
        &self,
        request: SyncRequest,
        batch_size: usize,
        fetch_prev_txouts: bool,
    ) -> Result<SyncResult, Error> {
        let full_scan_req = FullScanRequest::from_chain_tip(request.chain_tip.clone())
            .set_spks_for_keychain((), request.spks.enumerate().map(|(i, spk)| (i as u32, spk)));
        let mut full_scan_res = self.full_scan(full_scan_req, usize::MAX, batch_size, false)?;
        let (tip, latest_blocks) =
            fetch_tip_and_latest_blocks(&self.inner, request.chain_tip.clone())?;

        self.populate_with_txids(&mut full_scan_res.graph_update, request.txids)?;
        self.populate_with_outpoints(&mut full_scan_res.graph_update, request.outpoints)?;

        let chain_update = chain_update(
            tip,
            &latest_blocks,
            full_scan_res.graph_update.all_anchors(),
        )?;

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            self.fetch_prev_txout(&mut full_scan_res.graph_update)?;
        }

        Ok(SyncResult {
            chain_update,
            graph_update: full_scan_res.graph_update,
        })
    }

    /// Populate the `graph_update` with transactions/anchors associated with the given `spks`.
    ///
    /// Transactions that contains an output with requested spk, or spends form an output with
    /// requested spk will be added to `graph_update`. Anchors of the aforementioned transactions are
    /// also included.
    fn populate_with_spks(
        &self,
        graph_update: &mut TxGraph<BlockTime>,
        mut spks: impl Iterator<Item = (u32, ScriptBuf)>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<Option<u32>, Error> {
        let mut unused_spk_count = 0_usize;
        let mut last_active_index = Option::<u32>::None;

        loop {
            let spks = (0..batch_size)
                .map_while(|_| spks.next())
                .collect::<Vec<_>>();
            if spks.is_empty() {
                return Ok(last_active_index);
            }

            let spk_histories = self
                .inner
                .batch_script_get_history(spks.iter().map(|(_, s)| s.as_script()))?;

            for ((spk_index, _spk), spk_history) in spks.into_iter().zip(spk_histories) {
                if spk_history.is_empty() {
                    unused_spk_count = unused_spk_count.saturating_add(1);
                    if unused_spk_count >= stop_gap {
                        return Ok(last_active_index);
                    }
                    continue;
                } else {
                    last_active_index = Some(spk_index);
                    unused_spk_count = 0;
                }

                for tx_res in spk_history {
                    let _ = graph_update.insert_tx(self.fetch_tx(tx_res.tx_hash)?);
                    self.validate_merkle_for_anchor(graph_update, tx_res.tx_hash, tx_res.height)?;
                }
            }
        }
    }

    /// Populate the `graph_update` with associated transactions/anchors of `outpoints`.
    ///
    /// Transactions in which the outpoint resides, and transactions that spend from the outpoint are
    /// included. Anchors of the aforementioned transactions are included.
    fn populate_with_outpoints(
        &self,
        graph_update: &mut TxGraph<BlockTime>,
        outpoints: impl IntoIterator<Item = OutPoint>,
    ) -> Result<(), Error> {
        for outpoint in outpoints {
            let op_txid = outpoint.txid;
            let op_tx = self.fetch_tx(op_txid)?;
            let op_txout = match op_tx.output.get(outpoint.vout as usize) {
                Some(txout) => txout,
                None => continue,
            };
            debug_assert_eq!(op_tx.compute_txid(), op_txid);

            // attempt to find the following transactions (alongside their chain positions), and
            // add to our sparsechain `update`:
            let mut has_residing = false; // tx in which the outpoint resides
            let mut has_spending = false; // tx that spends the outpoint
            for res in self.inner.script_get_history(&op_txout.script_pubkey)? {
                if has_residing && has_spending {
                    break;
                }

                if !has_residing && res.tx_hash == op_txid {
                    has_residing = true;
                    let _ = graph_update.insert_tx(Arc::clone(&op_tx));
                    self.validate_merkle_for_anchor(graph_update, res.tx_hash, res.height)?;
                }

                if !has_spending && res.tx_hash != op_txid {
                    let res_tx = self.fetch_tx(res.tx_hash)?;
                    // we exclude txs/anchors that do not spend our specified outpoint(s)
                    has_spending = res_tx
                        .input
                        .iter()
                        .any(|txin| txin.previous_output == outpoint);
                    if !has_spending {
                        continue;
                    }
                    let _ = graph_update.insert_tx(Arc::clone(&res_tx));
                    self.validate_merkle_for_anchor(graph_update, res.tx_hash, res.height)?;
                }
            }
        }
        Ok(())
    }

    /// Populate the `graph_update` with transactions/anchors of the provided `txids`.
    fn populate_with_txids(
        &self,
        graph_update: &mut TxGraph<BlockTime>,
        txids: impl IntoIterator<Item = Txid>,
    ) -> Result<(), Error> {
        for txid in txids {
            let tx = match self.fetch_tx(txid) {
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
            if let Some(r) = self
                .inner
                .script_get_history(spk)?
                .into_iter()
                .find(|r| r.tx_hash == txid)
            {
                self.validate_merkle_for_anchor(graph_update, txid, r.height)?;
            }

            let _ = graph_update.insert_tx(tx);
        }
        Ok(())
    }

    // Helper function which checks if a transaction is confirmed by validating the merkle proof.
    // An anchor is inserted if the transaction is validated to be in a confirmed block.
    fn validate_merkle_for_anchor(
        &self,
        graph_update: &mut TxGraph<BlockTime>,
        txid: Txid,
        confirmation_height: i32,
    ) -> Result<(), Error> {
        if let Ok(merkle_res) = self
            .inner
            .transaction_get_merkle(&txid, confirmation_height as usize)
        {
            let mut header = self.fetch_header(merkle_res.block_height as u32)?;
            let mut is_confirmed_tx = electrum_client::utils::validate_merkle_proof(
                &txid,
                &header.merkle_root,
                &merkle_res,
            );

            // Merkle validation will fail if the header in `block_header_cache` is outdated, so we
            // want to check if there is a new header and validate against the new one.
            if !is_confirmed_tx {
                header = self.update_header(merkle_res.block_height as u32)?;
                is_confirmed_tx = electrum_client::utils::validate_merkle_proof(
                    &txid,
                    &header.merkle_root,
                    &merkle_res,
                );
            }

            if is_confirmed_tx {
                let blockid = BlockId {
                    height: merkle_res.block_height as u32,
                    hash: header.block_hash(),
                };
                let _ = graph_update.insert_anchor((txid, blockid), BlockTime::new(header.time));
            }
        }
        Ok(())
    }

    // Helper function which fetches the `TxOut`s of our relevant transactions' previous transactions,
    // which we do not have by default. This data is needed to calculate the transaction fee.
    fn fetch_prev_txout(&self, graph_update: &mut TxGraph<BlockTime>) -> Result<(), Error> {
        let full_txs: Vec<Arc<Transaction>> =
            graph_update.full_txs().map(|tx_node| tx_node.tx).collect();
        for tx in full_txs {
            for vin in &tx.input {
                let outpoint = vin.previous_output;
                let vout = outpoint.vout;
                let prev_tx = self.fetch_tx(outpoint.txid)?;
                let txout = prev_tx.output[vout as usize].clone();
                let _ = graph_update.insert_txout(outpoint, txout);
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
    };

    let agreement_height = agreement_cp.as_ref().map(CheckPoint::height);

    let new_tip = new_blocks
        .iter()
        // Prune `new_blocks` to only include blocks that are actually new.
        .filter(|(height, _)| Some(*<&u32>::clone(height)) > agreement_height)
        .map(|(height, hash)| BlockId {
            height: *height,
            hash: *hash,
        })
        .fold(agreement_cp, |prev_cp, block| {
            Some(match prev_cp {
                Some(cp) => cp.push(block).expect("must extend checkpoint"),
                None => CheckPoint::new(block),
            })
        })
        .expect("must have at least one checkpoint");

    Ok((new_tip, new_blocks))
}

// Add a corresponding checkpoint per anchor height if it does not yet exist. Checkpoints should not
// surpass `latest_blocks`.
fn chain_update<A>(
    mut tip: CheckPoint,
    latest_blocks: &BTreeMap<u32, BlockHash>,
    anchors: &BTreeMap<Anchor, A>,
) -> Result<CheckPoint, Error> {
    for (_txid, blockid) in anchors.keys() {
        let height = blockid.height;

        // Checkpoint uses the `BlockHash` from `latest_blocks` so that the hash will be consistent
        // in case of a re-org.
        if tip.get(height).is_none() && height <= tip.height() {
            let hash = match latest_blocks.get(&height) {
                Some(&hash) => hash,
                None => blockid.hash,
            };
            tip = tip.insert(BlockId { hash, height });
        }
    }
    Ok(tip)
}
