// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::network::message_blockdata::Inventory;
use bitcoin::{OutPoint, Transaction, Txid};

use rocksdb::{Options, SliceTransform, DB};

mod peer;
mod store;
mod sync;

use super::{Blockchain, Capability, OnlineBlockchain, Progress};
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{ScriptType, TransactionDetails, UTXO};
use crate::FeeRate;

use peer::*;
use store::*;
use sync::*;

pub use peer::{Mempool, Peer};

const SYNC_HEADERS_COST: f32 = 1.0;
const SYNC_FILTERS_COST: f32 = 11.6 * 1_000.0;
const PROCESS_BLOCKS_COST: f32 = 20_000.0;

#[derive(Debug)]
pub struct CompactFiltersBlockchain(Option<CompactFilters>);

impl CompactFiltersBlockchain {
    pub fn new<P: AsRef<Path>>(
        peers: Vec<Peer>,
        storage_dir: P,
        skip_blocks: Option<usize>,
    ) -> Result<Self, CompactFiltersError> {
        Ok(CompactFiltersBlockchain(Some(CompactFilters::new(
            peers,
            storage_dir,
            skip_blocks,
        )?)))
    }
}

#[derive(Debug)]
struct CompactFilters {
    peers: Vec<Arc<Peer>>,
    headers: Arc<ChainStore<Full>>,
    skip_blocks: Option<usize>,
}

impl CompactFilters {
    pub fn new<P: AsRef<Path>>(
        peers: Vec<Peer>,
        storage_dir: P,
        skip_blocks: Option<usize>,
    ) -> Result<Self, CompactFiltersError> {
        if peers.is_empty() {
            return Err(CompactFiltersError::NoPeers);
        }

        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(16));

        let network = peers[0].get_network();

        let cfs = DB::list_cf(&opts, &storage_dir).unwrap_or(vec!["default".to_string()]);
        let db = DB::open_cf(&opts, &storage_dir, &cfs)?;
        let headers = Arc::new(ChainStore::new(db, network)?);

        // try to recover partial snapshots
        for cf_name in &cfs {
            if !cf_name.starts_with("_headers:") {
                continue;
            }

            info!("Trying to recover: {:?}", cf_name);
            headers.recover_snapshot(cf_name)?;
        }

        Ok(CompactFilters {
            peers: peers.into_iter().map(Arc::new).collect(),
            headers,
            skip_blocks,
        })
    }

    fn process_tx<D: BatchDatabase>(
        &self,
        database: &mut D,
        tx: &Transaction,
        height: Option<u32>,
        timestamp: u64,
        internal_max_deriv: &mut u32,
        external_max_deriv: &mut u32,
    ) -> Result<(), Error> {
        let mut updates = database.begin_batch();

        let mut incoming: u64 = 0;
        let mut outgoing: u64 = 0;

        let mut inputs_sum: u64 = 0;
        let mut outputs_sum: u64 = 0;

        // look for our own inputs
        for (i, input) in tx.input.iter().enumerate() {
            if let Some(previous_output) = database.get_previous_output(&input.previous_output)? {
                inputs_sum += previous_output.value;

                if database.is_mine(&previous_output.script_pubkey)? {
                    outgoing += previous_output.value;

                    debug!("{} input #{} is mine, removing from utxo", tx.txid(), i);
                    updates.del_utxo(&input.previous_output)?;
                }
            }
        }

        for (i, output) in tx.output.iter().enumerate() {
            // to compute the fees later
            outputs_sum += output.value;

            // this output is ours, we have a path to derive it
            if let Some((script_type, child)) =
                database.get_path_from_script_pubkey(&output.script_pubkey)?
            {
                debug!("{} output #{} is mine, adding utxo", tx.txid(), i);
                updates.set_utxo(&UTXO {
                    outpoint: OutPoint::new(tx.txid(), i as u32),
                    txout: output.clone(),
                    is_internal: script_type.is_internal(),
                })?;
                incoming += output.value;

                if script_type == ScriptType::Internal && child > *internal_max_deriv {
                    *internal_max_deriv = child;
                } else if script_type == ScriptType::External && child > *external_max_deriv {
                    *external_max_deriv = child;
                }
            }
        }

        if incoming > 0 || outgoing > 0 {
            let tx = TransactionDetails {
                txid: tx.txid(),
                transaction: Some(tx.clone()),
                received: incoming,
                sent: outgoing,
                height,
                timestamp,
                fees: inputs_sum.checked_sub(outputs_sum).unwrap_or(0),
            };

            info!("Saving tx {}", tx.txid);
            updates.set_tx(&tx)?;
        }

        database.commit_batch(updates)?;

        Ok(())
    }
}

impl Blockchain for CompactFiltersBlockchain {
    fn offline() -> Self {
        CompactFiltersBlockchain(None)
    }

    fn is_online(&self) -> bool {
        self.0.is_some()
    }
}

impl OnlineBlockchain for CompactFiltersBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![Capability::FullHistory].into_iter().collect()
    }

    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        _stop_gap: Option<usize>, // TODO: move to electrum and esplora only
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        let inner = self.0.as_ref().ok_or(Error::OfflineClient)?;
        let first_peer = &inner.peers[0];

        let skip_blocks = inner.skip_blocks.unwrap_or(0);

        let cf_sync = Arc::new(CFSync::new(Arc::clone(&inner.headers), skip_blocks, 0x00)?);

        let initial_height = inner.headers.get_height()?;
        let total_bundles = (first_peer.get_version().start_height as usize)
            .checked_sub(skip_blocks)
            .map(|x| x / 1000)
            .unwrap_or(0)
            + 1;
        let expected_bundles_to_sync = total_bundles
            .checked_sub(cf_sync.pruned_bundles()?)
            .unwrap_or(0);

        let headers_cost = (first_peer.get_version().start_height as usize)
            .checked_sub(initial_height)
            .unwrap_or(0) as f32
            * SYNC_HEADERS_COST;
        let filters_cost = expected_bundles_to_sync as f32 * SYNC_FILTERS_COST;

        let total_cost = headers_cost + filters_cost + PROCESS_BLOCKS_COST;

        if let Some(snapshot) = sync::sync_headers(
            Arc::clone(&first_peer),
            Arc::clone(&inner.headers),
            |new_height| {
                let local_headers_cost =
                    new_height.checked_sub(initial_height).unwrap_or(0) as f32 * SYNC_HEADERS_COST;
                progress_update.update(
                    local_headers_cost / total_cost * 100.0,
                    Some(format!("Synced headers to {}", new_height)),
                )
            },
        )? {
            if snapshot.work()? > inner.headers.work()? {
                info!("Applying snapshot with work: {}", snapshot.work()?);
                inner.headers.apply_snapshot(snapshot)?;
            }
        }

        let synced_height = inner.headers.get_height()?;
        let buried_height = synced_height
            .checked_sub(sync::BURIED_CONFIRMATIONS)
            .unwrap_or(0);
        info!("Synced headers to height: {}", synced_height);

        cf_sync.prepare_sync(Arc::clone(&first_peer))?;

        let all_scripts = Arc::new(
            database
                .iter_script_pubkeys(None)?
                .into_iter()
                .map(|s| s.to_bytes())
                .collect::<Vec<_>>(),
        );

        let last_synced_block = Arc::new(Mutex::new(synced_height));
        let synced_bundles = Arc::new(AtomicUsize::new(0));
        let progress_update = Arc::new(Mutex::new(progress_update));

        let mut threads = Vec::with_capacity(inner.peers.len());
        for peer in &inner.peers {
            let cf_sync = Arc::clone(&cf_sync);
            let peer = Arc::clone(&peer);
            let headers = Arc::clone(&inner.headers);
            let all_scripts = Arc::clone(&all_scripts);
            let last_synced_block = Arc::clone(&last_synced_block);
            let progress_update = Arc::clone(&progress_update);
            let synced_bundles = Arc::clone(&synced_bundles);

            let thread = std::thread::spawn(move || {
                cf_sync.capture_thread_for_sync(
                    peer,
                    |block_hash, filter| {
                        if !filter
                            .match_any(block_hash, &mut all_scripts.iter().map(AsRef::as_ref))?
                        {
                            return Ok(false);
                        }

                        let block_height = headers.get_height_for(block_hash)?.unwrap_or(0);
                        let saved_correct_block = match headers.get_full_block(block_height)? {
                            Some(block) if &block.block_hash() == block_hash => true,
                            _ => false,
                        };

                        if saved_correct_block {
                            Ok(false)
                        } else {
                            let mut last_synced_block = last_synced_block.lock().unwrap();

                            // If we download a block older than `last_synced_block`, we update it so that
                            // we know to delete and re-process all txs starting from that height
                            if block_height < *last_synced_block {
                                *last_synced_block = block_height;
                            }

                            Ok(true)
                        }
                    },
                    |index| {
                        let synced_bundles = synced_bundles.fetch_add(1, Ordering::SeqCst);
                        let local_filters_cost = synced_bundles as f32 * SYNC_FILTERS_COST;
                        progress_update.lock().unwrap().update(
                            (headers_cost + local_filters_cost) / total_cost * 100.0,
                            Some(format!(
                                "Synced filters {} - {}",
                                index * 1000 + 1,
                                (index + 1) * 1000
                            )),
                        )
                    },
                )
            });

            threads.push(thread);
        }

        for t in threads {
            t.join().unwrap()?;
        }

        progress_update.lock().unwrap().update(
            (headers_cost + filters_cost) / total_cost * 100.0,
            Some("Processing downloaded blocks and mempool".into()),
        )?;

        // delete all txs newer than last_synced_block
        let last_synced_block = *last_synced_block.lock().unwrap();
        log::debug!(
            "Dropping transactions newer than `last_synced_block` = {}",
            last_synced_block
        );
        let mut updates = database.begin_batch();
        for details in database.iter_txs(false)? {
            match details.height {
                Some(height) if (height as usize) < last_synced_block => continue,
                _ => updates.del_tx(&details.txid, false)?,
            };
        }
        database.commit_batch(updates)?;

        first_peer.ask_for_mempool()?;

        let mut internal_max_deriv = 0;
        let mut external_max_deriv = 0;

        for (height, block) in inner.headers.iter_full_blocks()? {
            for tx in &block.txdata {
                inner.process_tx(
                    database,
                    tx,
                    Some(height as u32),
                    0,
                    &mut internal_max_deriv,
                    &mut external_max_deriv,
                )?;
            }
        }
        for tx in first_peer.get_mempool().iter_txs().iter() {
            inner.process_tx(
                database,
                tx,
                None,
                0,
                &mut internal_max_deriv,
                &mut external_max_deriv,
            )?;
        }

        let current_ext = database.get_last_index(ScriptType::External)?.unwrap_or(0);
        let first_ext_new = external_max_deriv as u32 + 1;
        if first_ext_new > current_ext {
            info!("Setting external index to {}", first_ext_new);
            database.set_last_index(ScriptType::External, first_ext_new)?;
        }

        let current_int = database.get_last_index(ScriptType::Internal)?.unwrap_or(0);
        let first_int_new = internal_max_deriv + 1;
        if first_int_new > current_int {
            info!("Setting internal index to {}", first_int_new);
            database.set_last_index(ScriptType::Internal, first_int_new)?;
        }

        info!("Dropping blocks until {}", buried_height);
        inner.headers.delete_blocks_until(buried_height)?;

        progress_update
            .lock()
            .unwrap()
            .update(100.0, Some("Done".into()))?;

        Ok(())
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let inner = self.0.as_ref().ok_or(Error::OfflineClient)?;

        Ok(inner.peers[0]
            .get_mempool()
            .get_tx(&Inventory::Transaction(*txid)))
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        let inner = self.0.as_ref().ok_or(Error::OfflineClient)?;
        inner.peers[0].broadcast_tx(tx.clone())?;

        Ok(())
    }

    fn get_height(&self) -> Result<u32, Error> {
        let inner = self.0.as_ref().ok_or(Error::OfflineClient)?;

        Ok(inner.headers.get_height()? as u32)
    }

    fn estimate_fee(&self, _target: usize) -> Result<FeeRate, Error> {
        // TODO
        Ok(FeeRate::default())
    }
}

#[derive(Debug)]
pub enum CompactFiltersError {
    InvalidResponse,
    InvalidHeaders,
    InvalidFilterHeader,
    InvalidFilter,
    MissingBlock,
    DataCorruption,

    NotConnected,
    Timeout,

    NoPeers,

    DB(rocksdb::Error),
    IO(std::io::Error),
    BIP158(bitcoin::util::bip158::Error),
    Time(std::time::SystemTimeError),

    Global(Box<crate::error::Error>),
}

impl fmt::Display for CompactFiltersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for CompactFiltersError {}

macro_rules! impl_error {
    ( $from:ty, $to:ident ) => {
        impl std::convert::From<$from> for CompactFiltersError {
            fn from(err: $from) -> Self {
                CompactFiltersError::$to(err)
            }
        }
    };
}

impl_error!(rocksdb::Error, DB);
impl_error!(std::io::Error, IO);
impl_error!(bitcoin::util::bip158::Error, BIP158);
impl_error!(std::time::SystemTimeError, Time);

impl From<crate::error::Error> for CompactFiltersError {
    fn from(err: crate::error::Error) -> Self {
        CompactFiltersError::Global(Box::new(err))
    }
}
