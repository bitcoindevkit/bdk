/*!
This models a how a sync happens where you have a server that you send your script pubkeys to and it
returns associated transactions i.e. electrum.
*/
#![allow(dead_code)]
use crate::{
    database::{BatchDatabase, BatchOperations, DatabaseUtils},
    ConfirmationTime, Error, KeychainKind, LocalUtxo, TransactionDetails,
};
use bitcoin::{OutPoint, Script, Transaction, TxOut, Txid};
use std::collections::{HashMap, HashSet, VecDeque};

struct State<'a, D> {
    db: &'a D,
    last_active_index: HashMap<KeychainKind, usize>,
    tx_needed: VecDeque<Txid>,
    conftime_needed: VecDeque<Txid>,
    observed_txs: Vec<TransactionDetails>,
}

/// A reqeust for on-chain information
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
        .collect();
    let state = State {
        db,
        last_active_index: HashMap::default(),
        conftime_needed: VecDeque::default(),
        observed_txs: vec![],
        tx_needed: VecDeque::default(),
    };

    Ok(Request::Script(ScriptReq {
        state,
        scripts_needed,
        script_index: 0,
        stop_gap,
        keychain,
        next_keychains: keychains,
        tx_interested: HashSet::default(),
        tx_conftime_interested: HashSet::default(),
    }))
}

pub struct ScriptReq<'a, D: BatchDatabase> {
    state: State<'a, D>,
    script_index: usize,
    scripts_needed: VecDeque<Script>,
    stop_gap: usize,
    keychain: KeychainKind,
    next_keychains: Vec<KeychainKind>,
    tx_interested: HashSet<Txid>,
    tx_conftime_interested: HashSet<Txid>,
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
        for txid_list in txids.iter() {
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
                                self.tx_conftime_interested.insert(*txid);
                            }
                            (Some(old_height), Some(new_height)) if old_height != *new_height => {
                                // The height of the tx has changed !? -- get the confirmation time.
                                self.tx_conftime_interested.insert(*txid);
                            }
                            (Some(_), None) => {
                                details.confirmation_time = None;
                                self.state.observed_txs.push(details);
                            }
                            _ => self.state.observed_txs.push(details),
                        }
                    }
                    None => {
                        // we've never seen it let's get the whole thing
                        self.tx_interested.insert(*txid);
                    }
                };
            }

            self.script_index += 1;
        }

        for _ in txids {
            self.scripts_needed.pop_front();
        }

        let last_active_index = self
            .state
            .last_active_index
            .get(&self.keychain)
            .map(|x| x + 1)
            .unwrap_or(0); // so no addresses active maps to 0

        Ok(
            if self.script_index > last_active_index + self.stop_gap
                || self.scripts_needed.is_empty()
            {
                // we're done here -- check if we need to do the next keychain
                if let Some(keychain) = self.next_keychains.pop() {
                    self.keychain = keychain;
                    self.script_index = 0;
                    self.scripts_needed = self
                        .state
                        .db
                        .iter_script_pubkeys(Some(keychain))?
                        .into_iter()
                        .collect();
                    Request::Script(self)
                } else {
                    self.state.conftime_needed = self.tx_conftime_interested.into_iter().collect();
                    self.state.tx_needed = self.tx_interested.into_iter().collect();
                    Request::Conftime(ConftimeReq { state: self.state })
                }
            } else {
                Request::Script(self)
            },
        )
    }
}

/// Next step is to get confirmation times for those we are interested in.
pub struct ConftimeReq<'a, D> {
    state: State<'a, D>,
}

impl<'a, D: BatchDatabase> ConftimeReq<'a, D> {
    pub fn request(&self) -> impl Iterator<Item = &Txid> + Clone {
        self.state.conftime_needed.iter()
    }

    pub fn satisfy(
        mut self,
        confirmation_times: Vec<Option<ConfirmationTime>>,
    ) -> Result<Request<'a, D>, Error> {
        let n = confirmation_times.len();
        for (confirmation_time, txid) in confirmation_times
            .into_iter()
            .zip(self.state.conftime_needed.iter())
        {
            if let Some(mut tx_details) = self.state.db.get_tx(txid, true)? {
                tx_details.confirmation_time = confirmation_time;
                self.state.observed_txs.push(tx_details);
            }
        }

        for _ in 0..n {
            self.state.conftime_needed.pop_front();
        }

        if self.state.conftime_needed.is_empty() {
            Ok(Request::Tx(TxReq { state: self.state }))
        } else {
            Ok(Request::Conftime(self))
        }
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
        tx_details: Vec<(Option<ConfirmationTime>, Vec<Option<TxOut>>, Transaction)>,
    ) -> Result<Request<'a, D>, Error> {
        let tx_details: Vec<TransactionDetails> = tx_details
            .into_iter()
            .zip(self.state.tx_needed.iter())
            .map(|((confirmation_time, vin, tx), txid)| {
                assert_eq!(tx.txid(), *txid);
                let mut sent: u64 = 0;
                let mut received: u64 = 0;
                let mut inputs_sum: u64 = 0;
                let mut outputs_sum: u64 = 0;

                for (txout, input) in vin.into_iter().zip(tx.input.iter()) {
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
                    confirmation_time,
                    fee: Some(fee),
                    verified: false,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        for tx_detail in tx_details {
            self.state.observed_txs.push(tx_detail);
            self.state.tx_needed.pop_front();
        }

        if !self.state.tx_needed.is_empty() {
            Ok(Request::Tx(self))
        } else {
            let existing_txs = self.state.db.iter_txs(false)?;
            let existing_txids: HashSet<Txid> = existing_txs.iter().map(|tx| tx.txid).collect();
            let observed_txs = make_txs_consistent(&self.state.observed_txs);
            let observed_txids: HashSet<Txid> = observed_txs.iter().map(|tx| tx.txid).collect();
            let txids_to_delete = existing_txids.difference(&observed_txids);
            let mut batch = self.state.db.begin_batch();

            // Delete old txs that no longer exist
            for txid in txids_to_delete {
                if let Some(raw_tx) = self.state.db.get_raw_tx(txid)? {
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

            // Set every tx we observed
            for observed_tx in &observed_txs {
                let tx = observed_tx
                    .transaction
                    .as_ref()
                    .expect("transaction will always be present here");
                for (i, output) in tx.output.iter().enumerate() {
                    if let Some((keychain, _)) = self
                        .state
                        .db
                        .get_path_from_script_pubkey(&output.script_pubkey)?
                    {
                        // add utxos we own from the new transactions we've seen.
                        batch.set_utxo(&LocalUtxo {
                            outpoint: OutPoint {
                                txid: observed_tx.txid,
                                vout: i as u32,
                            },
                            txout: output.clone(),
                            keychain,
                        })?;
                    }
                }
                batch.set_tx(observed_tx)?;
            }

            // we don't do this in the loop above since we may want to delete some of the utxos we
            // just added in case there are new tranasactions that spend form each other.
            for observed_tx in &observed_txs {
                let tx = observed_tx
                    .transaction
                    .as_ref()
                    .expect("transaction will always be present here");
                for input in &tx.input {
                    // Delete any spent utxos
                    batch.del_utxo(&input.previous_output)?;
                }
            }

            for (keychain, last_active_index) in self.state.last_active_index {
                batch.set_last_index(keychain, last_active_index as u32)?;
            }

            Ok(Request::Finish(batch))
        }
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
