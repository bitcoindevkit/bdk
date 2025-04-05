use bdk_core::{
    bitcoin::{block::Header, BlockHash, OutPoint, Transaction, Txid},
    collections::{BTreeMap, HashMap, HashSet},
    spk_client::{
        FullScanRequest, FullScanResponse, SpkWithExpectedTxids, SyncRequest, SyncResponse,
    },
    BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate,
};
use electrum_client::{ElectrumApi, Error, GetMerkleRes, HeaderNotification};
use std::sync::{Arc, Mutex};

/// We include a chain suffix of a certain length for the purpose of robustness.
const CHAIN_SUFFIX_LENGTH: u32 = 8;

/// Maximum batch size for Merkle proof requests
const MAX_MERKLE_BATCH_SIZE: usize = 100;

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
    /// The Merkle proof cache
    merkle_cache: Mutex<HashMap<(Txid, u32), GetMerkleRes>>,
}

impl<E: ElectrumApi> BdkElectrumClient<E> {
    /// Creates a new bdk client from a [`electrum_client::ElectrumApi`]
    pub fn new(client: E) -> Self {
        Self {
            inner: client,
            tx_cache: Default::default(),
            block_header_cache: Default::default(),
            merkle_cache: Default::default(),
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
    ///              see [`FullScanRequest`].
    /// - `stop_gap`: the full scan for each keychain stops after a gap of script pubkeys with no
    ///               associated transactions.
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///                 request.
    /// - `fetch_prev_txouts`: specifies whether we want previous `TxOut`s for fee calculation.
    ///                        Note that this requires additional calls to the Electrum server, but
    ///                        is necessary for calculating the fee on a transaction if your wallet
    ///                        does not own the inputs. Methods like [`Wallet.calculate_fee`] and
    ///                        [`Wallet.calculate_fee_rate`] will return a
    ///                        [`CalculateFeeError::MissingTxOut`] error if those `TxOut`s are not
    ///                        present in the transaction graph.
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
        for keychain in request.keychains() {
            let spks = request
                .iter_spks(keychain.clone())
                .map(|(spk_i, spk)| (spk_i, SpkWithExpectedTxids::from(spk)));
            if let Some(last_active_index) =
                self.populate_with_spks(start_time, &mut tx_update, spks, stop_gap, batch_size)?
            {
                last_active_indices.insert(keychain, last_active_index);
            }
        }

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            self.fetch_prev_txout(&mut tx_update)?;
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
    /// - `request`: struct with data required to perform a spk-based blockchain client sync,
    ///              see [`SyncRequest`]
    /// - `batch_size`: specifies the max number of script pubkeys to request for in a single batch
    ///                 request
    /// - `fetch_prev_txouts`: specifies whether we want previous `TxOut`s for fee calculation.
    ///                        Note that this requires additional calls to the Electrum server, but
    ///                        is necessary for calculating the fee on a transaction if your wallet
    ///                        does not own the inputs. Methods like [`Wallet.calculate_fee`] and
    ///                        [`Wallet.calculate_fee_rate`] will return a
    ///                        [`CalculateFeeError::MissingTxOut`] error if those `TxOut`s are not
    ///                        present in the transaction graph.
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
        self.populate_with_spks(
            start_time,
            &mut tx_update,
            request
                .iter_spks_with_expected_txids()
                .enumerate()
                .map(|(i, spk)| (i as u32, spk)),
            usize::MAX,
            batch_size,
        )?;
        self.populate_with_txids(start_time, &mut tx_update, request.iter_txids())?;
        self.populate_with_outpoints(start_time, &mut tx_update, request.iter_outpoints())?;

        // Fetch previous `TxOut`s for fee calculation if flag is enabled.
        if fetch_prev_txouts {
            self.fetch_prev_txout(&mut tx_update)?;
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
    ) -> Result<Option<u32>, Error> {
        let mut unused_spk_count = 0_usize;
        let mut last_active_index = Option::<u32>::None;
        let mut txs_to_validate = Vec::new();

        loop {
            let spks = (0..batch_size)
                .map_while(|_| spks_with_expected_txids.next())
                .collect::<Vec<_>>();
            if spks.is_empty() {
                break;
            }

            let spk_histories = self
                .inner
                .batch_script_get_history(spks.iter().map(|(_, s)| s.spk.as_script()))?;

            for ((spk_index, spk), spk_history) in spks.into_iter().zip(spk_histories) {
                if spk_history.is_empty() {
                    unused_spk_count = unused_spk_count.saturating_add(1);
                    if unused_spk_count >= stop_gap {
                        break;
                    }
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
                            txs_to_validate.push((tx_res.tx_hash, height));
                        }
                        _ => {
                            tx_update.seen_ats.insert((tx_res.tx_hash, start_time));
                        }
                    }
                }
            }
        }

        // Batch validate all collected transactions
        if !txs_to_validate.is_empty() {
            let proofs = self.batch_fetch_merkle_proofs(&txs_to_validate)?;
            self.batch_validate_merkle_proofs(tx_update, proofs)?;
        }

        Ok(last_active_index)
    }

    /// Populate the `tx_update` with associated transactions/anchors of `outpoints`.
    ///
    /// Transactions in which the outpoint resides, and transactions that spend from the outpoint are
    /// included. Anchors of the aforementioned transactions are included.
    fn populate_with_outpoints(
        &self,
        start_time: u64,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        outpoints: impl IntoIterator<Item = OutPoint>,
    ) -> Result<(), Error> {
        let mut txs_to_validate = Vec::new();

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
                    tx_update.txs.push(Arc::clone(&op_tx));
                    match res.height.try_into() {
                        // Returned heights 0 & -1 are reserved for unconfirmed txs.
                        Ok(height) if height > 0 => {
                            txs_to_validate.push((res.tx_hash, height));
                        }
                        _ => {
                            tx_update.seen_ats.insert((res.tx_hash, start_time));
                        }
                    }
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
                    tx_update.txs.push(Arc::clone(&res_tx));
                    match res.height.try_into() {
                        // Returned heights 0 & -1 are reserved for unconfirmed txs.
                        Ok(height) if height > 0 => {
                            txs_to_validate.push((res.tx_hash, height));
                        }
                        _ => {
                            tx_update.seen_ats.insert((res.tx_hash, start_time));
                        }
                    }
                }
            }
        }

        // Batch validate all collected transactions
        if !txs_to_validate.is_empty() {
            let proofs = self.batch_fetch_merkle_proofs(&txs_to_validate)?;
            self.batch_validate_merkle_proofs(tx_update, proofs)?;
        }

        Ok(())
    }

    /// Populate the `tx_update` with transactions/anchors of the provided `txids`.
    fn populate_with_txids(
        &self,
        start_time: u64,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        txids: impl IntoIterator<Item = Txid>,
    ) -> Result<(), Error> {
        let mut txs_to_validate = Vec::new();

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
                match r.height.try_into() {
                    // Returned heights 0 & -1 are reserved for unconfirmed txs.
                    Ok(height) if height > 0 => {
                        txs_to_validate.push((txid, height));
                    }
                    _ => {
                        tx_update.seen_ats.insert((r.tx_hash, start_time));
                    }
                }
            }

            tx_update.txs.push(tx);
        }

        // Batch validate all collected transactions
        if !txs_to_validate.is_empty() {
            let proofs = self.batch_fetch_merkle_proofs(&txs_to_validate)?;
            self.batch_validate_merkle_proofs(tx_update, proofs)?;
        }

        Ok(())
    }

    /// Batch fetch Merkle proofs for multiple transactions
    fn batch_fetch_merkle_proofs(
        &self,
        txs_with_heights: &[(Txid, usize)],
    ) -> Result<Vec<(Txid, GetMerkleRes)>, Error> {
        let mut results = Vec::with_capacity(txs_with_heights.len());
        let mut to_fetch = Vec::new();

        // Check cache first
        {
            let merkle_cache = self.merkle_cache.lock().unwrap();
            for &(txid, height) in txs_with_heights {
                if let Some(proof) = merkle_cache.get(&(txid, height as u32)) {
                    results.push((txid, proof.clone()));
                } else {
                    to_fetch.push((txid, height));
                }
            }
        }

        // Fetch missing proofs in batches
        for chunk in to_fetch.chunks(MAX_MERKLE_BATCH_SIZE) {
            for &(txid, height) in chunk {
                if let Ok(merkle_res) = self.inner.transaction_get_merkle(&txid, height) {
                    let mut cache = self.merkle_cache.lock().unwrap();
                    cache.insert((txid, height as u32), merkle_res.clone());
                    results.push((txid, merkle_res));
                }
            }
        }

        Ok(results)
    }

    /// Batch validate Merkle proofs
    fn batch_validate_merkle_proofs(
        &self,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        proofs: Vec<(Txid, GetMerkleRes)>,
    ) -> Result<(), Error> {
        // Pre-fetch all required headers
        let heights: HashSet<u32> = proofs
            .iter()
            .map(|(_, proof)| proof.block_height as u32)
            .collect();

        let mut headers = HashMap::new();
        for height in heights {
            headers.insert(height, self.fetch_header(height)?);
        }

        // Validate proofs
        for (txid, merkle_res) in proofs {
            let height = merkle_res.block_height as u32;
            let header = headers.get(&height).unwrap();

            let mut is_confirmed_tx = electrum_client::utils::validate_merkle_proof(
                &txid,
                &header.merkle_root,
                &merkle_res,
            );

            // Retry with updated header if validation fails
            if !is_confirmed_tx {
                let updated_header = self.update_header(height)?;
                headers.insert(height, updated_header);
                is_confirmed_tx = electrum_client::utils::validate_merkle_proof(
                    &txid,
                    &updated_header.merkle_root,
                    &merkle_res,
                );
            }

            if is_confirmed_tx {
                let header = headers.get(&height).unwrap();
                tx_update.anchors.insert((
                    ConfirmationBlockTime {
                        confirmation_time: header.time as u64,
                        block_id: BlockId {
                            height,
                            hash: header.block_hash(),
                        },
                    },
                    txid,
                ));
            }
        }

        Ok(())
    }

    // Replace the old validate_merkle_for_anchor with optimized batch version
    fn validate_merkle_for_anchor(
        &self,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        txid: Txid,
        confirmation_height: usize,
    ) -> Result<(), Error> {
        // Use the batch processing functions even for single tx
        let proofs = self.batch_fetch_merkle_proofs(&[(txid, confirmation_height)])?;
        self.batch_validate_merkle_proofs(tx_update, proofs)
    }

    // Helper function which fetches the `TxOut`s of our relevant transactions' previous transactions,
    // which we do not have by default. This data is needed to calculate the transaction fee.
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
                    let txout = prev_tx.output[vout as usize].clone();
                    let _ = tx_update.txouts.insert(outpoint, txout);
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
mod test {
    use crate::{bdk_electrum_client::TxUpdate, BdkElectrumClient};
    use bdk_chain::bitcoin::{OutPoint, Transaction, TxIn};
    use bdk_core::collections::BTreeMap;
    use bdk_testenv::{utils::new_tx, TestEnv};
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
}
