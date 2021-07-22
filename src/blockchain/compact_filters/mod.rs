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

//! Compact Filters
//!
//! This module contains a multithreaded implementation of an [`Blockchain`] backend that
//! uses BIP157 (aka "Neutrino") to populate the wallet's [database](crate::database::Database)
//! by downloading compact filters from the P2P network.
//!
//! Since there are currently very few peers "in the wild" that advertise the required service
//! flag, this implementation requires that one or more known peers are provided by the user.
//! No dns or other kinds of peer discovery are done internally.
//!
//! Moreover, this module doesn't currently support detecting and resolving conflicts between
//! messages received by different peers. Thus, it's recommended to use this module by only
//! connecting to a single peer at a time, optionally by opening multiple connections if it's
//! desirable to use multiple threads at once to sync in parallel.
//!
//! This is an **EXPERIMENTAL** feature, API and other major changes are expected.
//!
//! ## Example
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use bitcoin::*;
//! # use bdk::*;
//! # use bdk::blockchain::compact_filters::*;
//! let num_threads = 4;
//!
//! let mempool = Arc::new(Mempool::default());
//! let peers = (0..num_threads)
//!     .map(|_| {
//!         Peer::connect(
//!             "btcd-mainnet.lightning.computer:8333",
//!             Arc::clone(&mempool),
//!             Network::Bitcoin,
//!         )
//!     })
//!     .collect::<Result<_, _>>()?;
//! let blockchain = CompactFiltersBlockchain::new(peers, "./wallet-filters", Some(500_000))?;
//! # Ok::<(), CompactFiltersError>(())
//! ```

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::network::message_blockdata::Inventory;
use bitcoin::{Network, OutPoint, Transaction, Txid};

use rocksdb::{Options, SliceTransform, DB};

mod peer;
mod store;
mod sync;

use super::{Blockchain, Capability, ConfigurableBlockchain, Progress};
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{KeychainKind, LocalUtxo, TransactionDetails};
use crate::{ConfirmationTime, FeeRate};

use peer::*;
use store::*;
use sync::*;

pub use peer::{Mempool, Peer};

const SYNC_HEADERS_COST: f32 = 1.0;
const SYNC_FILTERS_COST: f32 = 11.6 * 1_000.0;
const PROCESS_BLOCKS_COST: f32 = 20_000.0;

/// Structure implementing the required blockchain traits
///
/// ## Example
/// See the [`blockchain::compact_filters`](crate::blockchain::compact_filters) module for a usage example.
#[derive(Debug)]
pub struct CompactFiltersBlockchain {
    peers: Vec<Arc<Peer>>,
    headers: Arc<ChainStore<Full>>,
    skip_blocks: Option<usize>,
}

impl CompactFiltersBlockchain {
    /// Construct a new instance given a list of peers, a path to store headers and block
    /// filters downloaded during the sync and optionally a number of blocks to ignore starting
    /// from the genesis while scanning for the wallet's outputs.
    ///
    /// For each [`Peer`] specified a new thread will be spawned to download and verify the filters
    /// in parallel. It's currently recommended to only connect to a single peer to avoid
    /// inconsistencies in the data returned, optionally with multiple connections in parallel to
    /// speed-up the sync process.
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

        let cfs = DB::list_cf(&opts, &storage_dir).unwrap_or_else(|_| vec!["default".to_string()]);
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

        Ok(CompactFiltersBlockchain {
            peers: peers.into_iter().map(Arc::new).collect(),
            headers,
            skip_blocks,
        })
    }

    /// Process a transaction by looking for inputs that spend from a UTXO in the database or
    /// outputs that send funds to a know script_pubkey.
    fn process_tx<D: BatchDatabase>(
        &self,
        database: &mut D,
        tx: &Transaction,
        height: Option<u32>,
        timestamp: Option<u64>,
        internal_max_deriv: &mut Option<u32>,
        external_max_deriv: &mut Option<u32>,
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
            if let Some((keychain, child)) =
                database.get_path_from_script_pubkey(&output.script_pubkey)?
            {
                debug!("{} output #{} is mine, adding utxo", tx.txid(), i);
                updates.set_utxo(&LocalUtxo {
                    outpoint: OutPoint::new(tx.txid(), i as u32),
                    txout: output.clone(),
                    keychain,
                })?;
                incoming += output.value;

                if keychain == KeychainKind::Internal
                    && (internal_max_deriv.is_none() || child > internal_max_deriv.unwrap_or(0))
                {
                    *internal_max_deriv = Some(child);
                } else if keychain == KeychainKind::External
                    && (external_max_deriv.is_none() || child > external_max_deriv.unwrap_or(0))
                {
                    *external_max_deriv = Some(child);
                }
            }
        }

        if incoming > 0 || outgoing > 0 {
            let tx = TransactionDetails {
                txid: tx.txid(),
                transaction: Some(tx.clone()),
                received: incoming,
                sent: outgoing,
                confirmation_time: ConfirmationTime::new(height, timestamp),
                verified: height.is_some(),
                fee: Some(inputs_sum.saturating_sub(outputs_sum)),
            };

            info!("Saving tx {}", tx.txid);
            updates.set_tx(&tx)?;
        }

        database.commit_batch(updates)?;

        Ok(())
    }
}

impl Blockchain for CompactFiltersBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![Capability::FullHistory].into_iter().collect()
    }

    #[allow(clippy::mutex_atomic)] // Mutex is easier to understand than a CAS loop.
    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        let first_peer = &self.peers[0];

        let skip_blocks = self.skip_blocks.unwrap_or(0);

        let cf_sync = Arc::new(CfSync::new(Arc::clone(&self.headers), skip_blocks, 0x00)?);

        let initial_height = self.headers.get_height()?;
        let total_bundles = (first_peer.get_version().start_height as usize)
            .checked_sub(skip_blocks)
            .map(|x| x / 1000)
            .unwrap_or(0)
            + 1;
        let expected_bundles_to_sync = total_bundles.saturating_sub(cf_sync.pruned_bundles()?);

        let headers_cost = (first_peer.get_version().start_height as usize)
            .saturating_sub(initial_height) as f32
            * SYNC_HEADERS_COST;
        let filters_cost = expected_bundles_to_sync as f32 * SYNC_FILTERS_COST;

        let total_cost = headers_cost + filters_cost + PROCESS_BLOCKS_COST;

        if let Some(snapshot) = sync::sync_headers(
            Arc::clone(&first_peer),
            Arc::clone(&self.headers),
            |new_height| {
                let local_headers_cost =
                    new_height.saturating_sub(initial_height) as f32 * SYNC_HEADERS_COST;
                progress_update.update(
                    local_headers_cost / total_cost * 100.0,
                    Some(format!("Synced headers to {}", new_height)),
                )
            },
        )? {
            if snapshot.work()? > self.headers.work()? {
                info!("Applying snapshot with work: {}", snapshot.work()?);
                self.headers.apply_snapshot(snapshot)?;
            }
        }

        let synced_height = self.headers.get_height()?;
        let buried_height = synced_height.saturating_sub(sync::BURIED_CONFIRMATIONS);
        info!("Synced headers to height: {}", synced_height);

        cf_sync.prepare_sync(Arc::clone(&first_peer))?;

        let all_scripts = Arc::new(
            database
                .iter_script_pubkeys(None)?
                .into_iter()
                .map(|s| s.to_bytes())
                .collect::<Vec<_>>(),
        );

        #[allow(clippy::mutex_atomic)]
        let last_synced_block = Arc::new(Mutex::new(synced_height));

        let synced_bundles = Arc::new(AtomicUsize::new(0));
        let progress_update = Arc::new(Mutex::new(progress_update));

        let mut threads = Vec::with_capacity(self.peers.len());
        for peer in &self.peers {
            let cf_sync = Arc::clone(&cf_sync);
            let peer = Arc::clone(&peer);
            let headers = Arc::clone(&self.headers);
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
                        let saved_correct_block = matches!(headers.get_full_block(block_height)?, Some(block) if &block.block_hash() == block_hash);

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
            match details.confirmation_time {
                Some(c) if (c.height as usize) < last_synced_block => continue,
                _ => updates.del_tx(&details.txid, false)?,
            };
        }
        database.commit_batch(updates)?;

        match first_peer.ask_for_mempool() {
            Err(CompactFiltersError::PeerBloomDisabled) => {
                log::warn!("Peer has BLOOM disabled, we can't ask for the mempool")
            }
            e => e?,
        };

        let mut internal_max_deriv = None;
        let mut external_max_deriv = None;

        for (height, block) in self.headers.iter_full_blocks()? {
            for tx in &block.txdata {
                self.process_tx(
                    database,
                    tx,
                    Some(height as u32),
                    None,
                    &mut internal_max_deriv,
                    &mut external_max_deriv,
                )?;
            }
        }
        for tx in first_peer.get_mempool().iter_txs().iter() {
            self.process_tx(
                database,
                tx,
                None,
                None,
                &mut internal_max_deriv,
                &mut external_max_deriv,
            )?;
        }

        let current_ext = database
            .get_last_index(KeychainKind::External)?
            .unwrap_or(0);
        let first_ext_new = external_max_deriv.map(|x| x + 1).unwrap_or(0);
        if first_ext_new > current_ext {
            info!("Setting external index to {}", first_ext_new);
            database.set_last_index(KeychainKind::External, first_ext_new)?;
        }

        let current_int = database
            .get_last_index(KeychainKind::Internal)?
            .unwrap_or(0);
        let first_int_new = internal_max_deriv.map(|x| x + 1).unwrap_or(0);
        if first_int_new > current_int {
            info!("Setting internal index to {}", first_int_new);
            database.set_last_index(KeychainKind::Internal, first_int_new)?;
        }

        info!("Dropping blocks until {}", buried_height);
        self.headers.delete_blocks_until(buried_height)?;

        progress_update
            .lock()
            .unwrap()
            .update(100.0, Some("Done".into()))?;

        Ok(())
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.peers[0]
            .get_mempool()
            .get_tx(&Inventory::Transaction(*txid)))
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        self.peers[0].broadcast_tx(tx.clone())?;

        Ok(())
    }

    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.headers.get_height()? as u32)
    }

    fn estimate_fee(&self, _target: usize) -> Result<FeeRate, Error> {
        // TODO
        Ok(FeeRate::default())
    }
}

/// Data to connect to a Bitcoin P2P peer
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct BitcoinPeerConfig {
    /// Peer address such as 127.0.0.1:18333
    pub address: String,
    /// Optional socks5 proxy
    pub socks5: Option<String>,
    /// Optional socks5 proxy credentials
    pub socks5_credentials: Option<(String, String)>,
}

/// Configuration for a [`CompactFiltersBlockchain`]
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct CompactFiltersBlockchainConfig {
    /// List of peers to try to connect to for asking headers and filters
    pub peers: Vec<BitcoinPeerConfig>,
    /// Network used
    pub network: Network,
    /// Storage dir to save partially downloaded headers and full blocks
    pub storage_dir: String,
    /// Optionally skip initial `skip_blocks` blocks (default: 0)
    pub skip_blocks: Option<usize>,
}

impl ConfigurableBlockchain for CompactFiltersBlockchain {
    type Config = CompactFiltersBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let mempool = Arc::new(Mempool::default());
        let peers = config
            .peers
            .iter()
            .map(|peer_conf| match &peer_conf.socks5 {
                None => Peer::connect(&peer_conf.address, Arc::clone(&mempool), config.network),
                Some(proxy) => Peer::connect_proxy(
                    peer_conf.address.as_str(),
                    proxy,
                    peer_conf
                        .socks5_credentials
                        .as_ref()
                        .map(|(a, b)| (a.as_str(), b.as_str())),
                    Arc::clone(&mempool),
                    config.network,
                ),
            })
            .collect::<Result<_, _>>()?;

        Ok(CompactFiltersBlockchain::new(
            peers,
            &config.storage_dir,
            config.skip_blocks,
        )?)
    }
}

/// An error that can occur during sync with a [`CompactFiltersBlockchain`]
#[derive(Debug)]
pub enum CompactFiltersError {
    /// A peer sent an invalid or unexpected response
    InvalidResponse,
    /// The headers returned are invalid
    InvalidHeaders,
    /// The compact filter headers returned are invalid
    InvalidFilterHeader,
    /// The compact filter returned is invalid
    InvalidFilter,
    /// The peer is missing a block in the valid chain
    MissingBlock,
    /// The data stored in the block filters storage are corrupted
    DataCorruption,

    /// A peer is not connected
    NotConnected,
    /// A peer took too long to reply to one of our messages
    Timeout,
    /// The peer doesn't advertise the [`BLOOM`](bitcoin::network::constants::ServiceFlags::BLOOM) service flag
    PeerBloomDisabled,

    /// No peers have been specified
    NoPeers,

    /// Internal database error
    Db(rocksdb::Error),
    /// Internal I/O error
    Io(std::io::Error),
    /// Invalid BIP158 filter
    Bip158(bitcoin::util::bip158::Error),
    /// Internal system time error
    Time(std::time::SystemTimeError),

    /// Wrapper for [`crate::error::Error`]
    Global(Box<crate::error::Error>),
}

impl fmt::Display for CompactFiltersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for CompactFiltersError {}

impl_error!(rocksdb::Error, Db, CompactFiltersError);
impl_error!(std::io::Error, Io, CompactFiltersError);
impl_error!(bitcoin::util::bip158::Error, Bip158, CompactFiltersError);
impl_error!(std::time::SystemTimeError, Time, CompactFiltersError);

impl From<crate::error::Error> for CompactFiltersError {
    fn from(err: crate::error::Error) -> Self {
        CompactFiltersError::Global(Box::new(err))
    }
}
