/*!
This models a how a sync happens where you have a server that you send your script pubkeys to and it
returns associated transactions i.e. electrum.
*/
#![allow(dead_code)]
use crate::{
    database::{BatchDatabase, BatchOperations, DatabaseUtils},
    error::MissingCachedScripts,
    wallet::time::Instant,
    BlockTime, Error, KeychainKind, LocalUtxo, TransactionDetails,
};
use bitcoin::{OutPoint, Script, Transaction, TxOut, Txid};
use log::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

/// A request for on-chain information
pub enum Request<'a, D: BatchDatabase> {
    /// A request for transactions related to script pubkeys.
    Script(ScriptReq<'a, D>),
    /// A request for confirmation times for some transactions.
    Conftime(ConftimeReq<'a, D>),
    /// A request for full transaction details of some transactions.
    Tx(TxReq<'a, D>),
    /// Requests are finished here's a batch database update to reflect data gathered.
    Finish(D::Batch),
}

/// starts a sync
pub fn start<D: BatchDatabase>(db: &D, stop_gap: usize) -> Result<Request<'_, D>, Error> {
    use rand::seq::SliceRandom;
    let mut keychains = vec![KeychainKind::Internal, KeychainKind::External];
    // shuffling improve privacy, the server doesn't know my first request is from my internal or external addresses
    keychains.shuffle(&mut rand::thread_rng());
    let keychain = keychains.pop().unwrap();
    let scripts_needed = db
        .iter_script_pubkeys(Some(keychain))?
        .into_iter()
        .collect::<VecDeque<_>>();
    let state = State::new(db);

    Ok(Request::Script(ScriptReq {
        state,
        initial_scripts_needed: scripts_needed.len(),
        scripts_needed,
        script_index: 0,
        stop_gap,
        keychain,
        next_keychains: keychains,
    }))
}

pub struct ScriptReq<'a, D: BatchDatabase> {
    state: State<'a, D>,
    script_index: usize,
    initial_scripts_needed: usize, // if this is 1, we assume the descriptor is not derivable
    scripts_needed: VecDeque<Script>,
    stop_gap: usize,
    keychain: KeychainKind,
    next_keychains: Vec<KeychainKind>,
}

/// The sync starts by returning script pubkeys we are interested in.
impl<'a, D: BatchDatabase> ScriptReq<'a, D> {
    pub fn request(&self) -> impl Iterator<Item = &Script> + Clone {
        self.scripts_needed.iter()
    }

    pub fn satisfy(
        mut self,
        // we want to know the txids assoiciated with the script and their height
        txids: Vec<Vec<(Txid, Option<u32>)>>,
    ) -> Result<Request<'a, D>, Error> {
        for (txid_list, script) in txids.iter().zip(self.scripts_needed.iter()) {
            debug!(
                "found {} transactions for script pubkey {}",
                txid_list.len(),
                script
            );
            if !txid_list.is_empty() {
                // the address is active
                self.state
                    .last_active_index
                    .insert(self.keychain, self.script_index);
            }

            for (txid, height) in txid_list {
                // have we seen this txid already?
                match self.state.db.get_tx(txid, true)? {
                    Some(mut details) => {
                        let old_height = details.confirmation_time.as_ref().map(|x| x.height);
                        match (old_height, height) {
                            (None, Some(_)) => {
                                // It looks like the tx has confirmed since we last saw it -- we
                                // need to know the confirmation time.
                                self.state.tx_missing_conftime.insert(*txid, details);
                            }
                            (Some(old_height), Some(new_height)) if old_height != *new_height => {
                                // The height of the tx has changed !? -- It's a reorg get the new confirmation time.
                                self.state.tx_missing_conftime.insert(*txid, details);
                            }
                            (Some(_), None) => {
                                // A re-org where the tx is not in the chain anymore.
                                details.confirmation_time = None;
                                self.state.finished_txs.push(details);
                            }
                            _ => self.state.finished_txs.push(details),
                        }
                    }
                    None => {
                        // we've never seen it let's get the whole thing
                        self.state.tx_needed.insert(*txid);
                    }
                };
            }

            self.script_index += 1;
        }

        self.scripts_needed.drain(..txids.len());

        // last active index: 0 => No last active
        let last = self
            .state
            .last_active_index
            .get(&self.keychain)
            .map(|&l| l + 1)
            .unwrap_or(0);
        // remaining scripts left to check
        let remaining = self.scripts_needed.len();
        // difference between current index and last active index
        let current_gap = self.script_index - last;

        // this is a hack to check whether the scripts are coming from a derivable descriptor
        // we assume for non-derivable descriptors, the initial script count is always 1
        let is_derivable = self.initial_scripts_needed > 1;

        debug!(
            "sync: last={}, remaining={}, diff={}, stop_gap={}",
            last, remaining, current_gap, self.stop_gap
        );

        if is_derivable {
            if remaining > 0 {
                // we still have scriptPubKeys to do requests for
                return Ok(Request::Script(self));
            }

            if last > 0 && current_gap < self.stop_gap {
                // current gap is not large enough to stop, but we are unable to keep checking since
                // we have exhausted cached scriptPubKeys, so return error
                let err = MissingCachedScripts {
                    last_count: self.script_index,
                    missing_count: self.stop_gap - current_gap,
                };
                return Err(Error::MissingCachedScripts(err));
            }

            // we have exhausted cached scriptPubKeys and found no txs, continue
        }

        debug!(
            "finished scanning for txs of keychain {:?} at index {:?}",
            self.keychain, last
        );

        if let Some(keychain) = self.next_keychains.pop() {
            // we still have another keychain to request txs with
            let scripts_needed = self
                .state
                .db
                .iter_script_pubkeys(Some(keychain))?
                .into_iter()
                .collect::<VecDeque<_>>();

            self.keychain = keychain;
            self.script_index = 0;
            self.initial_scripts_needed = scripts_needed.len();
            self.scripts_needed = scripts_needed;
            return Ok(Request::Script(self));
        }

        // We have finished requesting txids, let's get the actual txs.
        Ok(Request::Tx(TxReq { state: self.state }))
    }
}

/// Then we get full transactions
pub struct TxReq<'a, D> {
    state: State<'a, D>,
}

impl<'a, D: BatchDatabase> TxReq<'a, D> {
    pub fn request(&self) -> impl Iterator<Item = &Txid> + Clone {
        self.state.tx_needed.iter()
    }

    pub fn satisfy(
        mut self,
        tx_details: Vec<(Vec<Option<TxOut>>, Transaction)>,
    ) -> Result<Request<'a, D>, Error> {
        let tx_details: Vec<TransactionDetails> = tx_details
            .into_iter()
            .zip(self.state.tx_needed.iter())
            .map(|((vout, tx), txid)| {
                debug!("found tx_details for {}", txid);
                assert_eq!(tx.txid(), *txid);
                let mut sent: u64 = 0;
                let mut received: u64 = 0;
                let mut inputs_sum: u64 = 0;
                let mut outputs_sum: u64 = 0;

                for (txout, (_input_index, input)) in
                    vout.into_iter().zip(tx.input.iter().enumerate())
                {
                    let txout = match txout {
                        Some(txout) => txout,
                        None => {
                            // skip coinbase inputs
                            debug_assert!(
                                input.previous_output.is_null(),
                                "prevout should only be missing for coinbase"
                            );
                            continue;
                        }
                    };
                    // Verify this input if requested via feature flag
                    #[cfg(feature = "verify")]
                    {
                        use crate::wallet::verify::VerifyError;
                        let serialized_tx = bitcoin::consensus::serialize(&tx);
                        bitcoinconsensus::verify(
                            txout.script_pubkey.to_bytes().as_ref(),
                            txout.value,
                            &serialized_tx,
                            _input_index,
                        )
                        .map_err(VerifyError::from)?;
                    }
                    inputs_sum += txout.value;
                    if self.state.db.is_mine(&txout.script_pubkey)? {
                        sent += txout.value;
                    }
                }

                for out in &tx.output {
                    outputs_sum += out.value;
                    if self.state.db.is_mine(&out.script_pubkey)? {
                        received += out.value;
                    }
                }
                // we need to saturating sub since we want coinbase txs to map to 0 fee and
                // this subtraction will be negative for coinbase txs.
                let fee = inputs_sum.saturating_sub(outputs_sum);
                Result::<_, Error>::Ok(TransactionDetails {
                    txid: *txid,
                    transaction: Some(tx),
                    received,
                    sent,
                    // we're going to fill this in later
                    confirmation_time: None,
                    fee: Some(fee),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        for tx_detail in tx_details {
            self.state.tx_needed.remove(&tx_detail.txid);
            self.state
                .tx_missing_conftime
                .insert(tx_detail.txid, tx_detail);
        }

        if !self.state.tx_needed.is_empty() {
            Ok(Request::Tx(self))
        } else {
            Ok(Request::Conftime(ConftimeReq { state: self.state }))
        }
    }
}

/// Final step is to get confirmation times
pub struct ConftimeReq<'a, D> {
    state: State<'a, D>,
}

impl<'a, D: BatchDatabase> ConftimeReq<'a, D> {
    pub fn request(&self) -> impl Iterator<Item = &Txid> + Clone {
        self.state.tx_missing_conftime.keys()
    }

    pub fn satisfy(
        mut self,
        confirmation_times: Vec<Option<BlockTime>>,
    ) -> Result<Request<'a, D>, Error> {
        let conftime_needed = self
            .request()
            .cloned()
            .take(confirmation_times.len())
            .collect::<Vec<_>>();
        for (confirmation_time, txid) in confirmation_times.into_iter().zip(conftime_needed.iter())
        {
            debug!("confirmation time for {} was {:?}", txid, confirmation_time);
            if let Some(mut tx_details) = self.state.tx_missing_conftime.remove(txid) {
                tx_details.confirmation_time = confirmation_time;
                self.state.finished_txs.push(tx_details);
            }
        }

        if self.state.tx_missing_conftime.is_empty() {
            Ok(Request::Finish(self.state.into_db_update()?))
        } else {
            Ok(Request::Conftime(self))
        }
    }
}

struct State<'a, D> {
    db: &'a D,
    last_active_index: HashMap<KeychainKind, usize>,
    /// Transactions where we need to get the full details
    tx_needed: BTreeSet<Txid>,
    /// Transacitions that we know everything about
    finished_txs: Vec<TransactionDetails>,
    /// Transactions that discovered conftimes should be inserted into
    tx_missing_conftime: BTreeMap<Txid, TransactionDetails>,
    /// The start of the sync
    start_time: Instant,
    /// Missing number of scripts to cache per keychain
    missing_script_counts: HashMap<KeychainKind, usize>,
}

impl<'a, D: BatchDatabase> State<'a, D> {
    fn new(db: &'a D) -> Self {
        State {
            db,
            last_active_index: HashMap::default(),
            finished_txs: vec![],
            tx_needed: BTreeSet::default(),
            tx_missing_conftime: BTreeMap::default(),
            start_time: Instant::new(),
            missing_script_counts: HashMap::default(),
        }
    }
    fn into_db_update(self) -> Result<D::Batch, Error> {
        debug_assert!(self.tx_needed.is_empty() && self.tx_missing_conftime.is_empty());
        let existing_txs = self.db.iter_txs(false)?;
        let existing_txids: HashSet<Txid> = existing_txs.iter().map(|tx| tx.txid).collect();
        let finished_txs = make_txs_consistent(&self.finished_txs);
        let observed_txids: HashSet<Txid> = finished_txs.iter().map(|tx| tx.txid).collect();
        let txids_to_delete = existing_txids.difference(&observed_txids);

        // Ensure `last_active_index` does not decrement database's current state.
        let index_updates = self
            .last_active_index
            .iter()
            .map(|(keychain, sync_index)| {
                let sync_index = *sync_index as u32;
                let index_res = match self.db.get_last_index(*keychain) {
                    Ok(Some(db_index)) => Ok(std::cmp::max(db_index, sync_index)),
                    Ok(None) => Ok(sync_index),
                    Err(err) => Err(err),
                };
                index_res.map(|index| (*keychain, index))
            })
            .collect::<Result<Vec<(KeychainKind, u32)>, _>>()?;

        let mut batch = self.db.begin_batch();

        // Delete old txs that no longer exist
        for txid in txids_to_delete {
            if let Some(raw_tx) = self.db.get_raw_tx(txid)? {
                for i in 0..raw_tx.output.len() {
                    // Also delete any utxos from the txs that no longer exist.
                    let _ = batch.del_utxo(&OutPoint {
                        txid: *txid,
                        vout: i as u32,
                    })?;
                }
            } else {
                unreachable!("we should always have the raw tx");
            }
            batch.del_tx(txid, true)?;
        }

        let mut spent_utxos = HashSet::new();

        // track all the spent utxos
        for finished_tx in &finished_txs {
            let tx = finished_tx
                .transaction
                .as_ref()
                .expect("transaction will always be present here");
            for input in &tx.input {
                spent_utxos.insert(&input.previous_output);
            }
        }

        // set every utxo we observed, unless it's already spent
        // we don't do this in the loop above as we want to know all the spent outputs before
        // adding the non-spent to the batch in case there are new tranasactions
        // that spend form each other.
        for finished_tx in &finished_txs {
            let tx = finished_tx
                .transaction
                .as_ref()
                .expect("transaction will always be present here");
            for (i, output) in tx.output.iter().enumerate() {
                if let Some((keychain, _)) =
                    self.db.get_path_from_script_pubkey(&output.script_pubkey)?
                {
                    // add utxos we own from the new transactions we've seen.
                    let outpoint = OutPoint {
                        txid: finished_tx.txid,
                        vout: i as u32,
                    };

                    batch.set_utxo(&LocalUtxo {
                        outpoint,
                        txout: output.clone(),
                        keychain,
                        // Is this UTXO in the spent_utxos set?
                        is_spent: spent_utxos.get(&outpoint).is_some(),
                    })?;
                }
            }

            batch.set_tx(finished_tx)?;
        }

        // apply index updates
        for (keychain, new_index) in index_updates {
            debug!("updating index ({}, {})", keychain.as_byte(), new_index);
            batch.set_last_index(keychain, new_index)?;
        }

        info!(
            "finished setup, elapsed {:?}ms",
            self.start_time.elapsed().as_millis()
        );
        Ok(batch)
    }
}

/// Remove conflicting transactions -- tie breaking them by fee.
fn make_txs_consistent(txs: &[TransactionDetails]) -> Vec<&TransactionDetails> {
    let mut utxo_index: HashMap<OutPoint, &TransactionDetails> = HashMap::default();
    for tx in txs {
        for input in &tx.transaction.as_ref().unwrap().input {
            utxo_index
                .entry(input.previous_output)
                .and_modify(|existing| match (tx.fee, existing.fee) {
                    (Some(fee), Some(existing_fee)) if fee > existing_fee => *existing = tx,
                    (Some(_), None) => *existing = tx,
                    _ => { /* leave it the same */ }
                })
                .or_insert(tx);
        }
    }

    utxo_index
        .into_iter()
        .map(|(_, tx)| (tx.txid, tx))
        .collect::<HashMap<_, _>>()
        .into_iter()
        .map(|(_, tx)| tx)
        .collect()
}
