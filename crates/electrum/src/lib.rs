//! This crate is used for updating structures of the [`bdk_chain`] crate with data from electrum.
//!
//! The star of the show is the [`ElectrumExt::scan`] method, which scans for relevant blockchain
//! data (via electrum) and outputs an [`ElectrumUpdate`].
//!
//! An [`ElectrumUpdate`] only includes `txid`s and no full transactions. The caller is responsible
//! for obtaining full transactions before applying. This can be done with
//! these steps:
//!
//! 1. Determine which full transactions are missing. The method [`missing_full_txs`] of
//! [`ElectrumUpdate`] can be used.
//!
//! 2. Obtaining the full transactions. To do this via electrum, the method
//! [`batch_transaction_get`] can be used.
//!
//! Refer to [`bdk_electrum_example`] for a complete example.
//!
//! [`ElectrumClient::scan`]: ElectrumClient::scan
//! [`missing_full_txs`]: ElectrumUpdate::missing_full_txs
//! [`batch_transaction_get`]: ElectrumApi::batch_transaction_get
//! [`bdk_electrum_example`]: https://github.com/LLFourn/bdk_core_staging/tree/master/bdk_electrum_example

use bdk_chain::{
    bitcoin::{hashes::hex::FromHex, BlockHash, OutPoint, Script, Transaction, Txid},
    chain_graph::{self, ChainGraph},
    keychain::KeychainScan,
    sparse_chain::{self, ChainPosition, SparseChain},
    tx_graph::TxGraph,
    BlockId, ConfirmationTime, TxHeight,
};
use electrum_client::{Client, ElectrumApi, Error};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Debug,
};

pub mod v2;
pub use bdk_chain;
pub use electrum_client;

/// Trait to extend [`electrum_client::Client`] functionality.
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
pub trait ElectrumExt {
    /// Fetch the latest block height.
    fn get_tip(&self) -> Result<(u32, BlockHash), Error>;

    /// Scan the blockchain (via electrum) for the data specified. This returns a [`ElectrumUpdate`]
    /// which can be transformed into a [`KeychainScan`] after we find all the missing full
    /// transactions.
    ///
    /// - `local_chain`: the most recent block hashes present locally
    /// - `keychain_spks`: keychains that we want to scan transactions for
    /// - `txids`: transactions for which we want the updated [`ChainPosition`]s
    /// - `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to included in the update
    fn scan<K: Ord + Clone>(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<ElectrumUpdate<K, TxHeight>, Error>;

    /// Convenience method to call [`scan`] without requiring a keychain.
    ///
    /// [`scan`]: ElectrumExt::scan
    fn scan_without_keychain(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        misc_spks: impl IntoIterator<Item = Script>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        batch_size: usize,
    ) -> Result<SparseChain, Error> {
        let spk_iter = misc_spks
            .into_iter()
            .enumerate()
            .map(|(i, spk)| (i as u32, spk));

        self.scan(
            local_chain,
            [((), spk_iter)].into(),
            txids,
            outpoints,
            usize::MAX,
            batch_size,
        )
        .map(|u| u.chain_update)
    }
}

impl ElectrumExt for Client {
    fn get_tip(&self) -> Result<(u32, BlockHash), Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?
        self.block_headers_subscribe()
            .map(|data| (data.height as u32, data.header.block_hash()))
    }

    fn scan<K: Ord + Clone>(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<ElectrumUpdate<K, TxHeight>, Error> {
        let mut request_spks = keychain_spks
            .into_iter()
            .map(|(k, s)| {
                let iter = s.into_iter();
                (k, iter)
            })
            .collect::<BTreeMap<K, _>>();
        let mut scanned_spks = BTreeMap::<(K, u32), (Script, bool)>::new();

        let txids = txids.into_iter().collect::<Vec<_>>();
        let outpoints = outpoints.into_iter().collect::<Vec<_>>();

        let update = loop {
            let mut update = prepare_update(self, local_chain)?;

            if !request_spks.is_empty() {
                if !scanned_spks.is_empty() {
                    let mut scanned_spk_iter = scanned_spks
                        .iter()
                        .map(|(i, (spk, _))| (i.clone(), spk.clone()));
                    match populate_with_spks::<_, _>(
                        self,
                        &mut update,
                        &mut scanned_spk_iter,
                        stop_gap,
                        batch_size,
                    ) {
                        Err(InternalError::Reorg) => continue,
                        Err(InternalError::ElectrumError(e)) => return Err(e),
                        Ok(mut spks) => scanned_spks.append(&mut spks),
                    };
                }
                for (keychain, keychain_spks) in &mut request_spks {
                    match populate_with_spks::<u32, _>(
                        self,
                        &mut update,
                        keychain_spks,
                        stop_gap,
                        batch_size,
                    ) {
                        Err(InternalError::Reorg) => continue,
                        Err(InternalError::ElectrumError(e)) => return Err(e),
                        Ok(spks) => scanned_spks.extend(
                            spks.into_iter()
                                .map(|(spk_i, spk)| ((keychain.clone(), spk_i), spk)),
                        ),
                    };
                }
            }

            match populate_with_txids(self, &mut update, &mut txids.iter().cloned()) {
                Err(InternalError::Reorg) => continue,
                Err(InternalError::ElectrumError(e)) => return Err(e),
                Ok(_) => {}
            }

            match populate_with_outpoints(self, &mut update, &mut outpoints.iter().cloned()) {
                Err(InternalError::Reorg) => continue,
                Err(InternalError::ElectrumError(e)) => return Err(e),
                Ok(_txs) => { /* [TODO] cache full txs to reduce bandwidth */ }
            }

            // check for reorgs during scan process
            let our_tip = update
                .latest_checkpoint()
                .expect("update must have atleast one checkpoint");
            let server_blockhash = self.block_header(our_tip.height as usize)?.block_hash();
            if our_tip.hash != server_blockhash {
                continue; // reorg
            } else {
                break update;
            }
        };

        let last_active_index = request_spks
            .into_keys()
            .filter_map(|k| {
                scanned_spks
                    .range((k.clone(), u32::MIN)..=(k.clone(), u32::MAX))
                    .rev()
                    .find(|(_, (_, active))| *active)
                    .map(|((_, i), _)| (k, *i))
            })
            .collect::<BTreeMap<_, _>>();

        Ok(ElectrumUpdate {
            chain_update: update,
            last_active_indices: last_active_index,
        })
    }
}

/// The result of [`ElectrumExt::scan`].
pub struct ElectrumUpdate<K, P> {
    /// The internal [`SparseChain`] update.
    pub chain_update: SparseChain<P>,
    /// The last keychain script pubkey indices, which had transaction histories.
    pub last_active_indices: BTreeMap<K, u32>,
}

impl<K, P> Default for ElectrumUpdate<K, P> {
    fn default() -> Self {
        Self {
            chain_update: Default::default(),
            last_active_indices: Default::default(),
        }
    }
}

impl<K, P> AsRef<SparseChain<P>> for ElectrumUpdate<K, P> {
    fn as_ref(&self) -> &SparseChain<P> {
        &self.chain_update
    }
}

impl<K: Ord + Clone + Debug, P: ChainPosition> ElectrumUpdate<K, P> {
    /// Return a list of missing full transactions that are required to [`inflate_update`].
    ///
    /// [`inflate_update`]: bdk_chain::chain_graph::ChainGraph::inflate_update
    pub fn missing_full_txs<G>(&self, graph: G) -> Vec<&Txid>
    where
        G: AsRef<TxGraph>,
    {
        self.chain_update
            .txids()
            .filter(|(_, txid)| graph.as_ref().get_tx(*txid).is_none())
            .map(|(_, txid)| txid)
            .collect()
    }

    /// Transform the [`ElectrumUpdate`] into a [`KeychainScan`], which can be applied to a
    /// `tracker`.
    ///
    /// This will fail if there are missing full transactions not provided via `new_txs`.
    pub fn into_keychain_scan<CG>(
        self,
        new_txs: Vec<Transaction>,
        chain_graph: &CG,
    ) -> Result<KeychainScan<K, P>, chain_graph::NewError<P>>
    where
        CG: AsRef<ChainGraph<P>>,
    {
        Ok(KeychainScan {
            update: chain_graph
                .as_ref()
                .inflate_update(self.chain_update, new_txs)?,
            last_active_indices: self.last_active_indices,
        })
    }
}

impl<K: Ord + Clone + Debug> ElectrumUpdate<K, TxHeight> {
    /// Creates [`ElectrumUpdate<K, ConfirmationTime>`] from [`ElectrumUpdate<K, TxHeight>`].
    pub fn into_confirmation_time_update(
        self,
        client: &electrum_client::Client,
    ) -> Result<ElectrumUpdate<K, ConfirmationTime>, Error> {
        let heights = self
            .chain_update
            .range_txids_by_height(..TxHeight::Unconfirmed)
            .map(|(h, _)| match h {
                TxHeight::Confirmed(h) => *h,
                _ => unreachable!("already filtered out unconfirmed"),
            })
            .collect::<Vec<u32>>();

        let height_to_time = heights
            .clone()
            .into_iter()
            .zip(
                client
                    .batch_block_header(heights)?
                    .into_iter()
                    .map(|bh| bh.time as u64),
            )
            .collect::<HashMap<u32, u64>>();

        let mut new_update = SparseChain::<ConfirmationTime>::from_checkpoints(
            self.chain_update.range_checkpoints(..),
        );

        for &(tx_height, txid) in self.chain_update.txids() {
            let conf_time = match tx_height {
                TxHeight::Confirmed(height) => ConfirmationTime::Confirmed {
                    height,
                    time: height_to_time[&height],
                },
                TxHeight::Unconfirmed => ConfirmationTime::Unconfirmed { last_seen: 0 },
            };
            let _ = new_update.insert_tx(txid, conf_time).expect("must insert");
        }

        Ok(ElectrumUpdate {
            chain_update: new_update,
            last_active_indices: self.last_active_indices,
        })
    }
}

#[derive(Debug)]
enum InternalError {
    ElectrumError(Error),
    Reorg,
}

impl From<electrum_client::Error> for InternalError {
    fn from(value: electrum_client::Error) -> Self {
        Self::ElectrumError(value)
    }
}

fn get_tip(client: &Client) -> Result<(u32, BlockHash), Error> {
    // TODO: unsubscribe when added to the client, or is there a better call to use here?
    client
        .block_headers_subscribe()
        .map(|data| (data.height as u32, data.header.block_hash()))
}

/// Prepare an update sparsechain "template" based on the checkpoints of the `local_chain`.
fn prepare_update(
    client: &Client,
    local_chain: &BTreeMap<u32, BlockHash>,
) -> Result<SparseChain, Error> {
    let mut update = SparseChain::default();

    // Find the local chain block that is still there so our update can connect to the local chain.
    for (&existing_height, &existing_hash) in local_chain.iter().rev() {
        // TODO: a batch request may be safer, as a reorg that happens when we are obtaining
        //       `block_header`s will result in inconsistencies
        let current_hash = client.block_header(existing_height as usize)?.block_hash();
        let _ = update
            .insert_checkpoint(BlockId {
                height: existing_height,
                hash: current_hash,
            })
            .expect("This never errors because we are working with a fresh chain");

        if current_hash == existing_hash {
            break;
        }
    }

    // Insert the new tip so new transactions will be accepted into the sparsechain.
    let tip = {
        let (height, hash) = get_tip(client)?;
        BlockId { height, hash }
    };
    if let Err(failure) = update.insert_checkpoint(tip) {
        match failure {
            sparse_chain::InsertCheckpointError::HashNotMatching { .. } => {
                // There has been a re-org before we even begin scanning addresses.
                // Just recursively call (this should never happen).
                return prepare_update(client, local_chain);
            }
        }
    }

    Ok(update)
}

/// This atrocity is required because electrum thinks a height of 0 means "unconfirmed", but there is
/// such thing as a genesis block.
///
/// We contain an expectation for the genesis coinbase txid to always have a chain position of
/// [`TxHeight::Confirmed(0)`].
fn determine_tx_height(raw_height: i32, tip_height: u32, txid: Txid) -> TxHeight {
    if txid
        == Txid::from_hex("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
            .expect("must deserialize genesis coinbase txid")
    {
        return TxHeight::Confirmed(0);
    }
    match raw_height {
        h if h <= 0 => {
            debug_assert!(
                h == 0 || h == -1,
                "unexpected height ({}) from electrum server",
                h
            );
            TxHeight::Unconfirmed
        }
        h => {
            let h = h as u32;
            if h > tip_height {
                TxHeight::Unconfirmed
            } else {
                TxHeight::Confirmed(h)
            }
        }
    }
}

/// Populates the update [`SparseChain`] with related transactions and associated [`ChainPosition`]s
/// of the provided `outpoints` (this is the tx which contains the outpoint and the one spending the
/// outpoint).
///
/// Unfortunately, this is awkward to implement as electrum does not provide such an API. Instead, we
/// will get the tx history of the outpoint's spk and try to find the containing tx and the
/// spending tx.
fn populate_with_outpoints(
    client: &Client,
    update: &mut SparseChain,
    outpoints: &mut impl Iterator<Item = OutPoint>,
) -> Result<HashMap<Txid, Transaction>, InternalError> {
    let tip = update
        .latest_checkpoint()
        .expect("update must atleast have one checkpoint");

    let mut full_txs = HashMap::new();
    for outpoint in outpoints {
        let txid = outpoint.txid;
        let tx = client.transaction_get(&txid)?;
        debug_assert_eq!(tx.txid(), txid);
        let txout = match tx.output.get(outpoint.vout as usize) {
            Some(txout) => txout,
            None => continue,
        };

        // attempt to find the following transactions (alongside their chain positions), and
        // add to our sparsechain `update`:
        let mut has_residing = false; // tx in which the outpoint resides
        let mut has_spending = false; // tx that spends the outpoint
        for res in client.script_get_history(&txout.script_pubkey)? {
            if has_residing && has_spending {
                break;
            }

            if res.tx_hash == txid {
                if has_residing {
                    continue;
                }
                has_residing = true;
                full_txs.insert(res.tx_hash, tx.clone());
            } else {
                if has_spending {
                    continue;
                }
                let res_tx = match full_txs.get(&res.tx_hash) {
                    Some(tx) => tx,
                    None => {
                        let res_tx = client.transaction_get(&res.tx_hash)?;
                        full_txs.insert(res.tx_hash, res_tx);
                        full_txs.get(&res.tx_hash).expect("just inserted")
                    }
                };
                has_spending = res_tx
                    .input
                    .iter()
                    .any(|txin| txin.previous_output == outpoint);
                if !has_spending {
                    continue;
                }
            };

            let tx_height = determine_tx_height(res.height, tip.height, res.tx_hash);

            if let Err(failure) = update.insert_tx(res.tx_hash, tx_height) {
                match failure {
                    sparse_chain::InsertTxError::TxTooHigh { .. } => {
                        unreachable!("we should never encounter this as we ensured height <= tip");
                    }
                    sparse_chain::InsertTxError::TxMovedUnexpectedly { .. } => {
                        return Err(InternalError::Reorg);
                    }
                }
            }
        }
    }
    Ok(full_txs)
}

/// Populate an update [`SparseChain`] with transactions (and associated block positions) from
/// the given `txids`.
fn populate_with_txids(
    client: &Client,
    update: &mut SparseChain,
    txids: &mut impl Iterator<Item = Txid>,
) -> Result<(), InternalError> {
    let tip = update
        .latest_checkpoint()
        .expect("update must have atleast one checkpoint");
    for txid in txids {
        let tx = match client.transaction_get(&txid) {
            Ok(tx) => tx,
            Err(electrum_client::Error::Protocol(_)) => continue,
            Err(other_err) => return Err(other_err.into()),
        };

        let spk = tx
            .output
            .get(0)
            .map(|txo| &txo.script_pubkey)
            .expect("tx must have an output");

        let tx_height = match client
            .script_get_history(spk)?
            .into_iter()
            .find(|r| r.tx_hash == txid)
        {
            Some(r) => determine_tx_height(r.height, tip.height, r.tx_hash),
            None => continue,
        };

        if let Err(failure) = update.insert_tx(txid, tx_height) {
            match failure {
                sparse_chain::InsertTxError::TxTooHigh { .. } => {
                    unreachable!("we should never encounter this as we ensured height <= tip");
                }
                sparse_chain::InsertTxError::TxMovedUnexpectedly { .. } => {
                    return Err(InternalError::Reorg);
                }
            }
        }
    }
    Ok(())
}

/// Populate an update [`SparseChain`] with transactions (and associated block positions) from
/// the transaction history of the provided `spk`s.
fn populate_with_spks<I, S>(
    client: &Client,
    update: &mut SparseChain,
    spks: &mut S,
    stop_gap: usize,
    batch_size: usize,
) -> Result<BTreeMap<I, (Script, bool)>, InternalError>
where
    I: Ord + Clone,
    S: Iterator<Item = (I, Script)>,
{
    let tip = update.latest_checkpoint().map_or(0, |cp| cp.height);
    let mut unused_spk_count = 0_usize;
    let mut scanned_spks = BTreeMap::new();

    loop {
        let spks = (0..batch_size)
            .map_while(|_| spks.next())
            .collect::<Vec<_>>();
        if spks.is_empty() {
            return Ok(scanned_spks);
        }

        let spk_histories = client.batch_script_get_history(spks.iter().map(|(_, s)| s))?;

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

            for tx in spk_history {
                let tx_height = determine_tx_height(tx.height, tip, tx.tx_hash);

                if let Err(failure) = update.insert_tx(tx.tx_hash, tx_height) {
                    match failure {
                        sparse_chain::InsertTxError::TxTooHigh { .. } => {
                            unreachable!(
                                "we should never encounter this as we ensured height <= tip"
                            );
                        }
                        sparse_chain::InsertTxError::TxMovedUnexpectedly { .. } => {
                            return Err(InternalError::Reorg);
                        }
                    }
                }
            }
        }
    }
}
