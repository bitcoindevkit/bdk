use bdk_core::{
    bitcoin::{block::Header, BlockHash, OutPoint, ScriptBuf, Transaction, Txid},
    collections::{BTreeMap, HashMap},
    spk_client::{FullScanRequest, FullScanResponse, SyncRequest, SyncResponse},
    BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate,
};
use electrum_client::{ElectrumApi, Error, HeaderNotification};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

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

        let tip_and_latest_blocks = match request.chain_tip() {
            Some(chain_tip) => Some(fetch_tip_and_latest_blocks(&self.inner, chain_tip)?),
            None => None,
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        let mut last_active_indices = BTreeMap::<K, u32>::default();
        for keychain in request.keychains() {
            let spks = request.iter_spks(keychain.clone());
            if let Some(last_active_index) =
                self.populate_with_spks(&mut tx_update, spks, stop_gap, batch_size)?
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

        let tip_and_latest_blocks = match request.chain_tip() {
            Some(chain_tip) => Some(fetch_tip_and_latest_blocks(&self.inner, chain_tip)?),
            None => None,
        };

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        self.populate_with_spks(
            &mut tx_update,
            request
                .iter_spks()
                .enumerate()
                .map(|(i, spk)| (i as u32, spk)),
            usize::MAX,
            batch_size,
        )?;
        self.populate_with_txids(&mut tx_update, request.iter_txids())?;
        self.populate_with_outpoints(&mut tx_update, request.iter_outpoints())?;

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
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
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
                    tx_update.txs.push(self.fetch_tx(tx_res.tx_hash)?);
                    self.validate_merkle_for_anchor(tx_update, tx_res.tx_hash, tx_res.height)?;
                }
            }
        }
    }

    /// Populate the `tx_update` with associated transactions/anchors of `outpoints`.
    ///
    /// Transactions in which the outpoint resides, and transactions that spend from the outpoint are
    /// included. Anchors of the aforementioned transactions are included.
    fn populate_with_outpoints(
        &self,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
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
                    tx_update.txs.push(Arc::clone(&op_tx));
                    self.validate_merkle_for_anchor(tx_update, res.tx_hash, res.height)?;
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
                    self.validate_merkle_for_anchor(tx_update, res.tx_hash, res.height)?;
                }
            }
        }
        Ok(())
    }

    /// Populate the `tx_update` with transactions/anchors of the provided `txids`.
    fn populate_with_txids(
        &self,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
        txids: impl IntoIterator<Item = Txid>,
    ) -> Result<(), Error> {
        for txid in txids {
            let tx = match self.fetch_tx(txid) {
                Ok(tx) => tx,
                Err(electrum_client::Error::Protocol(_)) => continue,
                Err(other_err) => return Err(other_err),
            };

            if let Some(txo) = tx.output.first() {
                let spk = &txo.script_pubkey;

                // because of restrictions of the Electrum API, we have to use the `script_get_history`
                // call to get confirmation status of our transaction
                if let Some(r) = self
                    .inner
                    .script_get_history(spk)?
                    .into_iter()
                    .find(|r| r.tx_hash == txid)
                {
                    self.validate_merkle_for_anchor(tx_update, txid, r.height)?;
                }

                tx_update.txs.push(tx);
            }
        }
        Ok(())
    }

    // Helper function which checks if a transaction is confirmed by validating the merkle proof.
    // An anchor is inserted if the transaction is validated to be in a confirmed block.
    fn validate_merkle_for_anchor(
        &self,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
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

            // Validate that the length of the coinbase merkle path matches the length of the
            // transaction's merkle path, and verify the coinbase transaction's merkle proof. This
            // prevents a known brute-force exploit from inserting invalid transactions.
            if let Ok(txid_from_pos_res) = self
                .inner
                .txid_from_pos_with_merkle(merkle_res.block_height, 0)
            {
                // Construct the GetMerkleRes required for validating the merkle proof of the
                // coinbase transaction.
                let coinbase_merkle_res = &electrum_client::GetMerkleRes {
                    block_height: merkle_res.block_height,
                    pos: 0,
                    merkle: txid_from_pos_res.merkle.clone(),
                };

                is_confirmed_tx = txid_from_pos_res.merkle.len() == merkle_res.merkle.len()
                    && electrum_client::utils::validate_merkle_proof(
                        &txid_from_pos_res.tx_hash,
                        &header.merkle_root,
                        coinbase_merkle_res,
                    );
            }

            if is_confirmed_tx {
                tx_update.anchors.insert((
                    ConfirmationBlockTime {
                        confirmation_time: header.time as u64,
                        block_id: BlockId {
                            height: merkle_res.block_height as u32,
                            hash: header.block_hash(),
                        },
                    },
                    txid,
                ));
            }
        }
        Ok(())
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
    use bdk_chain::{
        bitcoin::{
            block::{Header, Version},
            pow::CompactTarget,
            BlockHash, Script, TxMerkleNode, Txid,
        },
        BlockId, ConfirmationBlockTime,
    };
    use bdk_core::collections::BTreeMap;
    use bdk_core::collections::BTreeSet;
    use bdk_testenv::{bitcoincore_rpc::jsonrpc::serde_json, utils::new_tx, TestEnv};
    use electrum_client::{
        Batch, ElectrumApi, Error, GetBalanceRes, GetHeadersRes, GetHistoryRes, GetMerkleRes,
        HeaderNotification, ListUnspentRes, Param, RawHeaderNotification, ScriptStatus,
        ServerFeaturesRes, TxidFromPosRes,
    };
    use std::{borrow::Borrow, str::FromStr};

    // Mock electrum client that can be used to set customized responses for testing.
    #[derive(Clone)]
    pub struct MockElectrumClient {
        pub merkle_res: Option<GetMerkleRes>,
        pub txid_from_pos_res: Option<TxidFromPosRes>,
        pub block_header: Option<Header>,
    }

    impl MockElectrumClient {
        pub fn new() -> Self {
            MockElectrumClient {
                merkle_res: None,
                txid_from_pos_res: None,
                block_header: None,
            }
        }

        pub fn set_merkle_res(&mut self, res: GetMerkleRes) {
            self.merkle_res = Some(res);
        }

        pub fn set_txid_from_pos_res(&mut self, res: TxidFromPosRes) {
            self.txid_from_pos_res = Some(res);
        }

        pub fn set_block_header(&mut self, header: Header) {
            self.block_header = Some(header);
        }
    }

    impl ElectrumApi for MockElectrumClient {
        fn transaction_get_merkle(
            &self,
            _txid: &Txid,
            _block_height: usize,
        ) -> Result<GetMerkleRes, Error> {
            self.merkle_res
                .clone()
                .ok_or_else(|| Error::Message("Missing GetMerkleRes".to_string()))
        }

        fn txid_from_pos_with_merkle(
            &self,
            _block_height: usize,
            _pos: usize,
        ) -> Result<TxidFromPosRes, Error> {
            self.txid_from_pos_res
                .clone()
                .ok_or_else(|| Error::Message("Missing TxidFromPosRes".to_string()))
        }

        fn block_header(&self, _height: usize) -> Result<Header, Error> {
            self.block_header
                .ok_or_else(|| Error::Message("Missing Header".to_string()))
        }

        fn block_headers_subscribe(&self) -> Result<HeaderNotification, Error> {
            unimplemented!()
        }

        fn block_headers_pop(&self) -> Result<Option<HeaderNotification>, Error> {
            unimplemented!()
        }

        fn transaction_get(&self, _txid: &Txid) -> Result<Transaction, Error> {
            unimplemented!()
        }

        fn batch_transaction_get<'t, I>(&self, _txids: I) -> Result<Vec<Transaction>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<&'t Txid>,
        {
            unimplemented!()
        }

        fn batch_block_header<I>(&self, _heights: I) -> Result<Vec<Header>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<u32>,
        {
            unimplemented!()
        }

        fn transaction_broadcast(&self, _tx: &Transaction) -> Result<Txid, Error> {
            unimplemented!()
        }

        fn raw_call(
            &self,
            _method_name: &str,
            _params: impl IntoIterator<Item = Param>,
        ) -> Result<serde_json::Value, Error> {
            unimplemented!()
        }

        fn batch_call(&self, _batch: &Batch) -> Result<Vec<serde_json::Value>, Error> {
            unimplemented!()
        }

        fn block_headers_subscribe_raw(&self) -> Result<RawHeaderNotification, Error> {
            unimplemented!()
        }

        fn block_headers_pop_raw(&self) -> Result<Option<RawHeaderNotification>, Error> {
            unimplemented!()
        }

        fn block_header_raw(&self, _height: usize) -> Result<Vec<u8>, Error> {
            unimplemented!()
        }

        fn block_headers(
            &self,
            _start_height: usize,
            _count: usize,
        ) -> Result<GetHeadersRes, Error> {
            unimplemented!()
        }

        fn estimate_fee(&self, _number: usize) -> Result<f64, Error> {
            unimplemented!()
        }

        fn relay_fee(&self) -> Result<f64, Error> {
            unimplemented!()
        }

        fn script_subscribe(&self, _script: &Script) -> Result<Option<ScriptStatus>, Error> {
            unimplemented!()
        }

        fn batch_script_subscribe<'s, I>(
            &self,
            _scripts: I,
        ) -> Result<Vec<Option<ScriptStatus>>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<&'s Script>,
        {
            unimplemented!()
        }

        fn script_unsubscribe(&self, _script: &Script) -> Result<bool, Error> {
            unimplemented!()
        }

        fn script_pop(&self, _script: &Script) -> Result<Option<ScriptStatus>, Error> {
            unimplemented!()
        }

        fn script_get_balance(&self, _script: &Script) -> Result<GetBalanceRes, Error> {
            unimplemented!()
        }

        fn batch_script_get_balance<'s, I>(&self, _scripts: I) -> Result<Vec<GetBalanceRes>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<&'s Script>,
        {
            unimplemented!()
        }

        fn script_get_history(&self, _script: &Script) -> Result<Vec<GetHistoryRes>, Error> {
            unimplemented!()
        }

        fn batch_script_get_history<'s, I>(
            &self,
            _scripts: I,
        ) -> Result<Vec<Vec<GetHistoryRes>>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<&'s Script>,
        {
            unimplemented!()
        }

        fn script_list_unspent(&self, _script: &Script) -> Result<Vec<ListUnspentRes>, Error> {
            unimplemented!()
        }

        fn batch_script_list_unspent<'s, I>(
            &self,
            _scripts: I,
        ) -> Result<Vec<Vec<ListUnspentRes>>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<&'s Script>,
        {
            unimplemented!()
        }

        fn transaction_get_raw(&self, _txid: &Txid) -> Result<Vec<u8>, Error> {
            unimplemented!()
        }

        fn batch_transaction_get_raw<'t, I>(&self, _txids: I) -> Result<Vec<Vec<u8>>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<&'t Txid>,
        {
            unimplemented!()
        }

        fn batch_block_header_raw<I>(&self, _heights: I) -> Result<Vec<Vec<u8>>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<u32>,
        {
            unimplemented!()
        }

        fn batch_estimate_fee<I>(&self, _numbers: I) -> Result<Vec<f64>, Error>
        where
            I: IntoIterator + Clone,
            I::Item: Borrow<usize>,
        {
            unimplemented!()
        }

        fn transaction_broadcast_raw(&self, _raw_tx: &[u8]) -> Result<Txid, Error> {
            unimplemented!()
        }

        fn txid_from_pos(&self, _height: usize, _tx_pos: usize) -> Result<Txid, Error> {
            unimplemented!()
        }

        fn server_features(&self) -> Result<ServerFeaturesRes, Error> {
            unimplemented!()
        }

        fn ping(&self) -> Result<(), Error> {
            unimplemented!()
        }
    }

    #[test]
    fn test_validate_coinbase_merkle() {
        let mut mock_client = MockElectrumClient::new();
        let txid =
            Txid::from_str("1f7ff3c407f33eabc8bec7d2cc230948f2249ec8e591bcf6f971ca9366c8788d")
                .unwrap();

        let mut merkle: Vec<[u8; 32]> = vec![
            [
                34, 65, 51, 64, 49, 139, 115, 189, 185, 246, 70, 225, 168, 193, 217, 195, 47, 66,
                179, 240, 153, 24, 114, 215, 144, 196, 212, 41, 39, 155, 246, 25,
            ],
            [
                185, 59, 215, 191, 138, 123, 233, 120, 174, 62, 233, 130, 153, 171, 102, 171, 32,
                174, 166, 60, 5, 151, 70, 218, 95, 110, 151, 77, 143, 97, 90, 19,
            ],
            [
                155, 2, 161, 113, 164, 35, 145, 2, 101, 98, 112, 120, 95, 13, 8, 1, 140, 128, 241,
                140, 31, 20, 214, 228, 131, 192, 252, 146, 211, 196, 43, 179,
            ],
            [
                155, 23, 18, 65, 102, 238, 135, 40, 232, 83, 40, 231, 13, 199, 40, 81, 96, 10, 207,
                111, 30, 82, 68, 142, 36, 21, 149, 101, 53, 107, 187, 161,
            ],
            [
                246, 103, 126, 61, 208, 239, 94, 152, 52, 41, 40, 241, 221, 154, 209, 196, 173,
                194, 211, 89, 64, 240, 12, 88, 222, 95, 216, 227, 168, 68, 142, 78,
            ],
            [
                232, 108, 108, 180, 65, 160, 85, 181, 159, 12, 193, 56, 210, 168, 187, 165, 11,
                129, 16, 101, 248, 6, 180, 198, 222, 52, 127, 99, 64, 243, 223, 236,
            ],
            [
                165, 160, 163, 233, 2, 89, 141, 233, 228, 173, 147, 169, 91, 158, 77, 9, 94, 201,
                159, 139, 187, 242, 76, 174, 39, 213, 9, 220, 65, 33, 74, 12,
            ],
            [
                175, 109, 105, 234, 23, 120, 195, 125, 108, 54, 156, 210, 197, 154, 114, 181, 77,
                97, 10, 190, 243, 19, 82, 33, 213, 95, 38, 52, 133, 230, 86, 132,
            ],
            [
                160, 171, 49, 37, 71, 5, 86, 200, 20, 163, 161, 165, 148, 193, 185, 175, 35, 164,
                193, 205, 16, 111, 30, 142, 193, 51, 220, 0, 81, 188, 25, 101,
            ],
            [
                172, 76, 240, 124, 147, 238, 233, 168, 132, 155, 126, 204, 0, 254, 83, 110, 122,
                23, 246, 16, 232, 57, 1, 97, 141, 68, 71, 2, 43, 69, 154, 86,
            ],
            [
                182, 99, 142, 10, 136, 90, 85, 59, 130, 238, 243, 117, 179, 135, 8, 127, 129, 195,
                84, 8, 158, 103, 12, 97, 253, 157, 71, 64, 32, 122, 52, 48,
            ],
            [
                24, 175, 170, 196, 213, 149, 80, 192, 109, 100, 53, 134, 173, 96, 83, 155, 143, 9,
                159, 113, 157, 161, 133, 77, 178, 75, 8, 64, 81, 90, 14, 28,
            ],
        ];

        // GetMerkleRes, the block header, and the subsequent txid and height used in
        // validate_merkle_for_anchor need to be valid in order for validate_merkle_proof to return
        // true. The merkle path here has a length of 12.
        let merkle_res = GetMerkleRes {
            block_height: 630000,
            pos: 68,
            merkle: merkle.clone(),
        };
        mock_client.set_merkle_res(merkle_res);

        let header = Header {
            version: Version::from_consensus(536870912),
            prev_blockhash: BlockHash::from_str(
                "0000000000000000000d656be18bb095db1b23bd797266b0ac3ba720b1962b1e",
            )
            .unwrap(),
            merkle_root: TxMerkleNode::from_str(
                "b191f5f973b9040e81c4f75f99c7e43c92010ba8654718e3dd1a4800851d300d",
            )
            .unwrap(),
            time: 1589225023,
            bits: CompactTarget::from_consensus(387021369),
            nonce: 2302182970,
        };
        mock_client.set_block_header(header);

        merkle = vec![
            [
                30, 10, 161, 245, 132, 125, 136, 198, 186, 138, 107, 216, 92, 22, 145, 81, 130,
                126, 200, 65, 121, 158, 105, 111, 38, 151, 38, 147, 144, 224, 5, 218,
            ],
            [
                16, 245, 30, 176, 235, 72, 58, 108, 38, 26, 236, 17, 138, 235, 231, 182, 149, 25,
                101, 238, 238, 238, 175, 148, 82, 183, 204, 41, 131, 36, 145, 113,
            ],
            [
                29, 159, 187, 22, 126, 103, 119, 114, 136, 137, 177, 221, 72, 147, 36, 215, 242,
                178, 230, 226, 20, 184, 65, 147, 173, 67, 157, 246, 254, 116, 225, 109,
            ],
            [
                42, 17, 94, 57, 204, 128, 27, 169, 61, 18, 111, 66, 247, 92, 121, 129, 98, 234,
                218, 114, 203, 51, 249, 145, 65, 118, 179, 227, 246, 42, 134, 171,
            ],
            [
                240, 44, 170, 24, 11, 6, 71, 160, 94, 49, 22, 162, 53, 38, 98, 40, 92, 206, 125,
                142, 162, 48, 39, 146, 237, 246, 108, 98, 200, 149, 97, 121,
            ],
            [
                121, 61, 243, 180, 64, 58, 107, 239, 85, 215, 79, 75, 134, 172, 122, 119, 9, 41,
                16, 34, 74, 75, 13, 111, 208, 114, 184, 166, 32, 171, 186, 83,
            ],
            [
                59, 29, 13, 95, 135, 131, 4, 175, 46, 34, 124, 69, 188, 129, 223, 161, 59, 210, 99,
                129, 174, 189, 181, 202, 107, 209, 236, 102, 109, 146, 142, 86,
            ],
            [
                175, 109, 105, 234, 23, 120, 195, 125, 108, 54, 156, 210, 197, 154, 114, 181, 77,
                97, 10, 190, 243, 19, 82, 33, 213, 95, 38, 52, 133, 230, 86, 132,
            ],
            [
                160, 171, 49, 37, 71, 5, 86, 200, 20, 163, 161, 165, 148, 193, 185, 175, 35, 164,
                193, 205, 16, 111, 30, 142, 193, 51, 220, 0, 81, 188, 25, 101,
            ],
            [
                172, 76, 240, 124, 147, 238, 233, 168, 132, 155, 126, 204, 0, 254, 83, 110, 122,
                23, 246, 16, 232, 57, 1, 97, 141, 68, 71, 2, 43, 69, 154, 86,
            ],
            [
                182, 99, 142, 10, 136, 90, 85, 59, 130, 238, 243, 117, 179, 135, 8, 127, 129, 195,
                84, 8, 158, 103, 12, 97, 253, 157, 71, 64, 32, 122, 52, 48,
            ],
            [
                24, 175, 170, 196, 213, 149, 80, 192, 109, 100, 53, 134, 173, 96, 83, 155, 143, 9,
                159, 113, 157, 161, 133, 77, 178, 75, 8, 64, 81, 90, 14, 28,
            ],
        ];

        // We first setup TxidFromPosRes to return a valid result.
        let txid_from_pos_res = TxidFromPosRes {
            tx_hash: Txid::from_str(
                "cc2ca076fd04c2aeed6d02151c447ced3d09be6fb4d4ef36cb5ed4e7a3260566",
            )
            .unwrap(),
            merkle: merkle.clone(),
        };
        mock_client.set_txid_from_pos_res(txid_from_pos_res);

        let client = BdkElectrumClient::new(mock_client.clone());
        let mut tx_update = TxUpdate::default();

        // With valid data, we are expecting validate_merkle_for_anchor to insert anchor data.
        let _ = client.validate_merkle_for_anchor(&mut tx_update, txid, 630000);
        assert_eq!(
            tx_update.anchors,
            BTreeSet::from([(
                ConfirmationBlockTime {
                    block_id: BlockId {
                        height: 630000,
                        hash: BlockHash::from_str(
                            "000000000000000000024bead8df69990852c202db0e0097c1a12ea637d7e96d"
                        )
                        .unwrap(),
                    },
                    confirmation_time: 1589225023
                },
                Txid::from_str("1f7ff3c407f33eabc8bec7d2cc230948f2249ec8e591bcf6f971ca9366c8788d")
                    .unwrap()
            )])
        );

        // Now we setup TxidFromPosRes to return invalid data. The merkle path length will now
        // return a value of 13.
        merkle.push([0; 32]);
        assert_eq!(merkle.len(), 13);

        let txid_from_pos_res = TxidFromPosRes {
            tx_hash: new_tx(0).compute_txid(),
            merkle,
        };
        mock_client.set_txid_from_pos_res(txid_from_pos_res);

        let client = BdkElectrumClient::new(mock_client);
        tx_update = TxUpdate::default();

        // Because the transaction merkle path length is now different from the coinbase merkle path
        // length, we are expecting validate_merkle_for_anchor to not insert any anchors.
        let _ = client.validate_merkle_for_anchor(&mut tx_update, txid, 630000);
        assert_eq!(tx_update.anchors, BTreeSet::default());
    }
    use std::sync::Arc;

    #[cfg(feature = "default")]
    #[test]
    fn test_populate_with_txids_without_output() {
        let env = TestEnv::new().unwrap();
        let electrum_client =
            electrum_client::Client::new(env.electrsd.electrum_url.as_str()).unwrap();
        let client = BdkElectrumClient::new(electrum_client);

        // Setup transaction with no outputs.
        let tx = new_tx(0);

        // Populate tx_cache with `tx` to make it fetchable.
        client.populate_tx_cache(vec![tx.clone()]);

        // Test that populate_with_txids does not panic or process a tx with no output.
        let mut tx_update = TxUpdate::default();
        let _ = client.populate_with_txids(&mut tx_update, vec![tx.compute_txid()]);

        assert_eq!(tx_update.txs, Vec::new());
    }

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
        let mut tx_update = TxUpdate {
            txs: vec![Arc::new(coinbase_tx)],
            ..Default::default()
        };
        assert!(client.fetch_prev_txout(&mut tx_update).is_ok());

        // Ensure that the txouts are empty.
        assert_eq!(tx_update.txouts, BTreeMap::default());
    }
}
