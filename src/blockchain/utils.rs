use std::cmp;
use std::collections::{HashSet, VecDeque};
use std::convert::TryFrom;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{Address, Network, OutPoint, Script, Transaction, Txid};

use super::*;
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{ScriptType, TransactionDetails, UTXO};
use crate::wallet::utils::ChunksIterator;

#[derive(Debug)]
pub struct ELSGetHistoryRes {
    pub height: i32,
    pub tx_hash: Txid,
}

#[derive(Debug)]
pub struct ELSListUnspentRes {
    pub height: usize,
    pub tx_hash: Txid,
    pub tx_pos: usize,
}

/// Implements the synchronization logic for an Electrum-like client.
#[maybe_async]
pub trait ElectrumLikeSync {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSGetHistoryRes>>, Error>;

    fn els_batch_script_list_unspent<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSListUnspentRes>>, Error>;

    fn els_transaction_get(&self, txid: &Txid) -> Result<Transaction, Error>;

    // Provided methods down here...

    fn electrum_like_setup<D: BatchDatabase + DatabaseUtils, P: Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        _progress_update: P,
    ) -> Result<(), Error> {
        // TODO: progress

        let stop_gap = stop_gap.unwrap_or(20);
        let batch_query_size = 20;

        // check unconfirmed tx, delete so they are retrieved later
        let mut del_batch = database.begin_batch();
        for tx in database.iter_txs(false)? {
            if tx.height.is_none() {
                del_batch.del_tx(&tx.txid, false)?;
            }
        }
        database.commit_batch(del_batch)?;

        // maximum derivation index for a change address that we've seen during sync
        let mut change_max_deriv = 0;

        let mut already_checked: HashSet<Script> = HashSet::new();
        let mut to_check_later = VecDeque::with_capacity(batch_query_size);

        // insert the first chunk
        let mut iter_scriptpubkeys = database
            .iter_script_pubkeys(Some(ScriptType::External))?
            .into_iter();
        let chunk: Vec<Script> = iter_scriptpubkeys.by_ref().take(batch_query_size).collect();
        for item in chunk.into_iter().rev() {
            to_check_later.push_front(item);
        }

        let mut iterating_external = true;
        let mut index = 0;
        let mut last_found = 0;
        while !to_check_later.is_empty() {
            trace!("to_check_later size {}", to_check_later.len());

            let until = cmp::min(to_check_later.len(), batch_query_size);
            let chunk: Vec<Script> = to_check_later.drain(..until).collect();
            let call_result = maybe_await!(self.els_batch_script_get_history(chunk.iter()))?;

            for (script, history) in chunk.into_iter().zip(call_result.into_iter()) {
                trace!("received history for {:?}, size {}", script, history.len());

                if !history.is_empty() {
                    last_found = index;

                    let mut check_later_scripts = maybe_await!(self.check_history(
                        database,
                        script,
                        history,
                        &mut change_max_deriv
                    ))?
                    .into_iter()
                    .filter(|x| already_checked.insert(x.clone()))
                    .collect();
                    to_check_later.append(&mut check_later_scripts);
                }

                index += 1;
            }

            match iterating_external {
                true if index - last_found >= stop_gap => iterating_external = false,
                true => {
                    trace!("pushing one more batch from `iter_scriptpubkeys`. index = {}, last_found = {}, stop_gap = {}", index, last_found, stop_gap);

                    let chunk: Vec<Script> =
                        iter_scriptpubkeys.by_ref().take(batch_query_size).collect();
                    for item in chunk.into_iter().rev() {
                        to_check_later.push_front(item);
                    }
                }
                _ => {}
            }
        }

        // check utxo
        // TODO: try to minimize network requests and re-use scripts if possible
        let mut batch = database.begin_batch();
        for chunk in ChunksIterator::new(database.iter_utxos()?.into_iter(), batch_query_size) {
            let scripts: Vec<_> = chunk.iter().map(|u| &u.txout.script_pubkey).collect();
            let call_result = maybe_await!(self.els_batch_script_list_unspent(scripts))?;

            // check which utxos are actually still unspent
            for (utxo, list_unspent) in chunk.into_iter().zip(call_result.iter()) {
                debug!(
                    "outpoint {:?} is unspent for me, list unspent is {:?}",
                    utxo.outpoint, list_unspent
                );

                let mut spent = true;
                for unspent in list_unspent {
                    let res_outpoint = OutPoint::new(unspent.tx_hash, unspent.tx_pos as u32);
                    if utxo.outpoint == res_outpoint {
                        spent = false;
                        break;
                    }
                }
                if spent {
                    info!("{} not anymore unspent, removing", utxo.outpoint);
                    batch.del_utxo(&utxo.outpoint)?;
                }
            }
        }

        let current_ext = database.get_last_index(ScriptType::External)?.unwrap_or(0);
        let first_ext_new = last_found as u32 + 1;
        if first_ext_new > current_ext {
            info!("Setting external index to {}", first_ext_new);
            database.set_last_index(ScriptType::External, first_ext_new)?;
        }

        let current_int = database.get_last_index(ScriptType::Internal)?.unwrap_or(0);
        let first_int_new = change_max_deriv + 1;
        if first_int_new > current_int {
            info!("Setting internal index to {}", first_int_new);
            database.set_last_index(ScriptType::Internal, first_int_new)?;
        }

        database.commit_batch(batch)?;

        Ok(())
    }

    fn check_tx_and_descendant<D: DatabaseUtils + BatchDatabase>(
        &self,
        database: &mut D,
        txid: &Txid,
        height: Option<u32>,
        cur_script: &Script,
        change_max_deriv: &mut u32,
    ) -> Result<Vec<Script>, Error> {
        debug!(
            "check_tx_and_descendant of {}, height: {:?}, script: {}",
            txid, height, cur_script
        );
        let mut updates = database.begin_batch();
        let tx = match database.get_tx(&txid, true)? {
            // TODO: do we need the raw?
            Some(mut saved_tx) => {
                // update the height if it's different (in case of reorg)
                if saved_tx.height != height {
                    info!(
                        "updating height from {:?} to {:?} for tx {}",
                        saved_tx.height, height, txid
                    );
                    saved_tx.height = height;
                    updates.set_tx(&saved_tx)?;
                }

                debug!("already have {} in db, returning the cached version", txid);

                // unwrap since we explicitly ask for the raw_tx, if it's not present something
                // went wrong
                saved_tx.transaction.unwrap()
            }
            None => maybe_await!(self.els_transaction_get(&txid))?,
        };

        let mut incoming: u64 = 0;
        let mut outgoing: u64 = 0;

        // look for our own inputs
        for (i, input) in tx.input.iter().enumerate() {
            // the fact that we visit addresses in a BFS fashion starting from the external addresses
            // should ensure that this query is always consistent (i.e. when we get to call this all
            // the transactions at a lower depth have already been indexed, so if an outpoint is ours
            // we are guaranteed to have it in the db).
            if let Some(previous_output) = database.get_previous_output(&input.previous_output)? {
                if database.is_mine(&previous_output.script_pubkey)? {
                    outgoing += previous_output.value;

                    debug!("{} input #{} is mine, removing from utxo", txid, i);
                    updates.del_utxo(&input.previous_output)?;
                }
            }
        }

        let mut to_check_later = vec![];
        for (i, output) in tx.output.iter().enumerate() {
            // this output is ours, we have a path to derive it
            if let Some((script_type, child)) =
                database.get_path_from_script_pubkey(&output.script_pubkey)?
            {
                debug!("{} output #{} is mine, adding utxo", txid, i);
                updates.set_utxo(&UTXO {
                    outpoint: OutPoint::new(tx.txid(), i as u32),
                    txout: output.clone(),
                    is_internal: script_type.is_internal(),
                })?;
                incoming += output.value;

                if output.script_pubkey != *cur_script {
                    debug!("{} output #{} script {} was not current script, adding script to be checked later", txid, i, output.script_pubkey);
                    to_check_later.push(output.script_pubkey.clone())
                }

                // derive as many change addrs as external addresses that we've seen
                if script_type == ScriptType::Internal && child > *change_max_deriv {
                    *change_max_deriv = child;
                }
            }
        }

        let tx = TransactionDetails {
            txid: tx.txid(),
            transaction: Some(tx),
            received: incoming,
            sent: outgoing,
            height,
            timestamp: 0,
        };
        info!("Saving tx {}", txid);
        updates.set_tx(&tx)?;

        database.commit_batch(updates)?;

        Ok(to_check_later)
    }

    fn check_history<D: DatabaseUtils + BatchDatabase>(
        &self,
        database: &mut D,
        script_pubkey: Script,
        txs: Vec<ELSGetHistoryRes>,
        change_max_deriv: &mut u32,
    ) -> Result<Vec<Script>, Error> {
        let mut to_check_later = Vec::new();

        debug!(
            "history of {} script {} has {} tx",
            Address::from_script(&script_pubkey, Network::Testnet).unwrap(),
            script_pubkey,
            txs.len()
        );

        for tx in txs {
            let height: Option<u32> = match tx.height {
                0 | -1 => None,
                x => u32::try_from(x).ok(),
            };

            to_check_later.extend_from_slice(&maybe_await!(self.check_tx_and_descendant(
                database,
                &tx.tx_hash,
                height,
                &script_pubkey,
                change_max_deriv,
            ))?);
        }

        Ok(to_check_later)
    }
}
