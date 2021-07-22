// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use std::collections::{HashMap, HashSet};

#[allow(unused_imports)]
use log::{debug, error, info, trace};
use rand::seq::SliceRandom;
use rand::thread_rng;

use bitcoin::{BlockHeader, OutPoint, Script, Transaction, Txid};

use super::*;
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{ConfirmationTime, KeychainKind, LocalUtxo, TransactionDetails};
use crate::wallet::time::Instant;
use crate::wallet::utils::ChunksIterator;

#[derive(Debug)]
pub struct ElsGetHistoryRes {
    pub height: i32,
    pub tx_hash: Txid,
}

/// Implements the synchronization logic for an Electrum-like client.
#[maybe_async]
pub trait ElectrumLikeSync {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script> + Clone>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ElsGetHistoryRes>>, Error>;

    fn els_batch_transaction_get<'s, I: IntoIterator<Item = &'s Txid> + Clone>(
        &self,
        txids: I,
    ) -> Result<Vec<Transaction>, Error>;

    fn els_batch_block_header<I: IntoIterator<Item = u32> + Clone>(
        &self,
        heights: I,
    ) -> Result<Vec<BlockHeader>, Error>;

    // Provided methods down here...

    fn electrum_like_setup<D: BatchDatabase, P: Progress>(
        &self,
        stop_gap: usize,
        db: &mut D,
        _progress_update: P,
    ) -> Result<(), Error> {
        // TODO: progress
        let start = Instant::new();
        debug!("start setup");

        let chunk_size = stop_gap;

        let mut history_txs_id = HashSet::new();
        let mut txid_height = HashMap::new();
        let mut max_indexes = HashMap::new();

        let mut wallet_chains = vec![KeychainKind::Internal, KeychainKind::External];
        // shuffling improve privacy, the server doesn't know my first request is from my internal or external addresses
        wallet_chains.shuffle(&mut thread_rng());
        // download history of our internal and external script_pubkeys
        for keychain in wallet_chains.iter() {
            let script_iter = db.iter_script_pubkeys(Some(*keychain))?.into_iter();

            for (i, chunk) in ChunksIterator::new(script_iter, stop_gap).enumerate() {
                // TODO if i == last, should create another chunk of addresses in db
                let call_result: Vec<Vec<ElsGetHistoryRes>> =
                    maybe_await!(self.els_batch_script_get_history(chunk.iter()))?;
                let max_index = call_result
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.first().map(|_| i as u32))
                    .max();
                if let Some(max) = max_index {
                    max_indexes.insert(keychain, max + (i * chunk_size) as u32);
                }
                let flattened: Vec<ElsGetHistoryRes> = call_result.into_iter().flatten().collect();
                debug!("#{} of {:?} results:{}", i, keychain, flattened.len());
                if flattened.is_empty() {
                    // Didn't find anything in the last `stop_gap` script_pubkeys, breaking
                    break;
                }

                for el in flattened {
                    // el.height = -1 means unconfirmed with unconfirmed parents
                    // el.height =  0 means unconfirmed with confirmed parents
                    // but we treat those tx the same
                    if el.height <= 0 {
                        txid_height.insert(el.tx_hash, None);
                    } else {
                        txid_height.insert(el.tx_hash, Some(el.height as u32));
                    }
                    history_txs_id.insert(el.tx_hash);
                }
            }
        }

        // saving max indexes
        info!("max indexes are: {:?}", max_indexes);
        for keychain in wallet_chains.iter() {
            if let Some(index) = max_indexes.get(keychain) {
                db.set_last_index(*keychain, *index)?;
            }
        }

        // get db status
        let txs_details_in_db: HashMap<Txid, TransactionDetails> = db
            .iter_txs(false)?
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect();
        let txs_raw_in_db: HashMap<Txid, Transaction> = db
            .iter_raw_txs()?
            .into_iter()
            .map(|tx| (tx.txid(), tx))
            .collect();
        let utxos_deps = utxos_deps(db, &txs_raw_in_db)?;

        // download new txs and headers
        let new_txs = maybe_await!(self.download_and_save_needed_raw_txs(
            &history_txs_id,
            &txs_raw_in_db,
            chunk_size,
            db
        ))?;
        let new_timestamps = maybe_await!(self.download_needed_headers(
            &txid_height,
            &txs_details_in_db,
            chunk_size
        ))?;

        let mut batch = db.begin_batch();

        // save any tx details not in db but in history_txs_id or with different height/timestamp
        for txid in history_txs_id.iter() {
            let height = txid_height.get(txid).cloned().flatten();
            let timestamp = new_timestamps.get(txid).cloned();
            if let Some(tx_details) = txs_details_in_db.get(txid) {
                // check if tx height matches, otherwise updates it. timestamp is not in the if clause
                // because we are not asking headers for confirmed tx we know about
                if tx_details.confirmation_time.as_ref().map(|c| c.height) != height {
                    let confirmation_time = ConfirmationTime::new(height, timestamp);
                    let mut new_tx_details = tx_details.clone();
                    new_tx_details.confirmation_time = confirmation_time;
                    batch.set_tx(&new_tx_details)?;
                }
            } else {
                save_transaction_details_and_utxos(
                    txid,
                    db,
                    timestamp,
                    height,
                    &mut batch,
                    &utxos_deps,
                )?;
            }
        }

        // remove any tx details in db but not in history_txs_id
        for txid in txs_details_in_db.keys() {
            if !history_txs_id.contains(txid) {
                batch.del_tx(txid, false)?;
            }
        }

        // remove any spent utxo
        for new_tx in new_txs.iter() {
            for input in new_tx.input.iter() {
                batch.del_utxo(&input.previous_output)?;
            }
        }

        db.commit_batch(batch)?;
        info!("finish setup, elapsed {:?}ms", start.elapsed().as_millis());

        Ok(())
    }

    /// download txs identified by `history_txs_id` and theirs previous outputs if not already present in db
    fn download_and_save_needed_raw_txs<D: BatchDatabase>(
        &self,
        history_txs_id: &HashSet<Txid>,
        txs_raw_in_db: &HashMap<Txid, Transaction>,
        chunk_size: usize,
        db: &mut D,
    ) -> Result<Vec<Transaction>, Error> {
        let mut txs_downloaded = vec![];
        let txids_raw_in_db: HashSet<Txid> = txs_raw_in_db.keys().cloned().collect();
        let txids_to_download: Vec<&Txid> = history_txs_id.difference(&txids_raw_in_db).collect();
        if !txids_to_download.is_empty() {
            info!("got {} txs to download", txids_to_download.len());
            txs_downloaded.extend(maybe_await!(self.download_and_save_in_chunks(
                txids_to_download,
                chunk_size,
                db,
            ))?);
            let mut prev_txids = HashSet::new();
            let mut txids_downloaded = HashSet::new();
            for tx in txs_downloaded.iter() {
                txids_downloaded.insert(tx.txid());
                // add every previous input tx, but skip coinbase
                for input in tx.input.iter().filter(|i| !i.previous_output.is_null()) {
                    prev_txids.insert(input.previous_output.txid);
                }
            }
            let already_present: HashSet<Txid> =
                txids_downloaded.union(&txids_raw_in_db).cloned().collect();
            let prev_txs_to_download: Vec<&Txid> =
                prev_txids.difference(&already_present).collect();
            info!("{} previous txs to download", prev_txs_to_download.len());
            txs_downloaded.extend(maybe_await!(self.download_and_save_in_chunks(
                prev_txs_to_download,
                chunk_size,
                db,
            ))?);
        }

        Ok(txs_downloaded)
    }

    /// download headers at heights in `txid_height` if tx details not already present, returns a map Txid -> timestamp
    fn download_needed_headers(
        &self,
        txid_height: &HashMap<Txid, Option<u32>>,
        txs_details_in_db: &HashMap<Txid, TransactionDetails>,
        chunk_size: usize,
    ) -> Result<HashMap<Txid, u64>, Error> {
        let mut txid_timestamp = HashMap::new();
        let txid_in_db_with_conf: HashSet<_> = txs_details_in_db
            .values()
            .filter_map(|details| details.confirmation_time.as_ref().map(|_| details.txid))
            .collect();
        let needed_txid_height: HashMap<&Txid, u32> = txid_height
            .iter()
            .filter(|(t, _)| !txid_in_db_with_conf.contains(*t))
            .filter_map(|(t, o)| o.map(|h| (t, h)))
            .collect();
        let needed_heights: HashSet<u32> = needed_txid_height.values().cloned().collect();
        if !needed_heights.is_empty() {
            info!("{} headers to download for timestamp", needed_heights.len());
            let mut height_timestamp: HashMap<u32, u64> = HashMap::new();
            for chunk in ChunksIterator::new(needed_heights.into_iter(), chunk_size) {
                let call_result: Vec<BlockHeader> =
                    maybe_await!(self.els_batch_block_header(chunk.clone()))?;
                height_timestamp.extend(
                    chunk
                        .into_iter()
                        .zip(call_result.iter().map(|h| h.time as u64)),
                );
            }
            for (txid, height) in needed_txid_height {
                let timestamp = height_timestamp
                    .get(&height)
                    .ok_or_else(|| Error::Generic("timestamp missing".to_string()))?;
                txid_timestamp.insert(*txid, *timestamp);
            }
        }

        Ok(txid_timestamp)
    }

    fn download_and_save_in_chunks<D: BatchDatabase>(
        &self,
        to_download: Vec<&Txid>,
        chunk_size: usize,
        db: &mut D,
    ) -> Result<Vec<Transaction>, Error> {
        let mut txs_downloaded = vec![];
        for chunk in ChunksIterator::new(to_download.into_iter(), chunk_size) {
            let call_result: Vec<Transaction> =
                maybe_await!(self.els_batch_transaction_get(chunk))?;
            let mut batch = db.begin_batch();
            for new_tx in call_result.iter() {
                batch.set_raw_tx(new_tx)?;
            }
            db.commit_batch(batch)?;
            txs_downloaded.extend(call_result);
        }

        Ok(txs_downloaded)
    }
}

fn save_transaction_details_and_utxos<D: BatchDatabase>(
    txid: &Txid,
    db: &mut D,
    timestamp: Option<u64>,
    height: Option<u32>,
    updates: &mut dyn BatchOperations,
    utxo_deps: &HashMap<OutPoint, OutPoint>,
) -> Result<(), Error> {
    let tx = db.get_raw_tx(txid)?.ok_or(Error::TransactionNotFound)?;

    let mut incoming: u64 = 0;
    let mut outgoing: u64 = 0;

    let mut inputs_sum: u64 = 0;
    let mut outputs_sum: u64 = 0;

    // look for our own inputs
    for input in tx.input.iter() {
        // skip coinbase inputs
        if input.previous_output.is_null() {
            continue;
        }

        // We already downloaded all previous output txs in the previous step
        if let Some(previous_output) = db.get_previous_output(&input.previous_output)? {
            inputs_sum += previous_output.value;

            if db.is_mine(&previous_output.script_pubkey)? {
                outgoing += previous_output.value;
            }
        } else {
            // The input is not ours, but we still need to count it for the fees
            let tx = db
                .get_raw_tx(&input.previous_output.txid)?
                .ok_or(Error::TransactionNotFound)?;
            inputs_sum += tx.output[input.previous_output.vout as usize].value;
        }

        // removes conflicting UTXO if any (generated from same inputs, like for example RBF)
        if let Some(outpoint) = utxo_deps.get(&input.previous_output) {
            updates.del_utxo(outpoint)?;
        }
    }

    for (i, output) in tx.output.iter().enumerate() {
        // to compute the fees later
        outputs_sum += output.value;

        // this output is ours, we have a path to derive it
        if let Some((keychain, _child)) = db.get_path_from_script_pubkey(&output.script_pubkey)? {
            debug!("{} output #{} is mine, adding utxo", txid, i);
            updates.set_utxo(&LocalUtxo {
                outpoint: OutPoint::new(tx.txid(), i as u32),
                txout: output.clone(),
                keychain,
            })?;

            incoming += output.value;
        }
    }

    let tx_details = TransactionDetails {
        txid: tx.txid(),
        transaction: Some(tx),
        received: incoming,
        sent: outgoing,
        confirmation_time: ConfirmationTime::new(height, timestamp),
        fee: Some(inputs_sum.saturating_sub(outputs_sum)), /* if the tx is a coinbase, fees would be negative */
        verified: height.is_some(),
    };
    updates.set_tx(&tx_details)?;

    Ok(())
}

/// returns utxo dependency as the inputs needed for the utxo to exist
/// `tx_raw_in_db` must contains utxo's generating txs or errors witt [crate::Error::TransactionNotFound]
fn utxos_deps<D: BatchDatabase>(
    db: &mut D,
    tx_raw_in_db: &HashMap<Txid, Transaction>,
) -> Result<HashMap<OutPoint, OutPoint>, Error> {
    let utxos = db.iter_utxos()?;
    let mut utxos_deps = HashMap::new();
    for utxo in utxos {
        let from_tx = tx_raw_in_db
            .get(&utxo.outpoint.txid)
            .ok_or(Error::TransactionNotFound)?;
        for input in from_tx.input.iter() {
            utxos_deps.insert(input.previous_output, utxo.outpoint);
        }
    }
    Ok(utxos_deps)
}
