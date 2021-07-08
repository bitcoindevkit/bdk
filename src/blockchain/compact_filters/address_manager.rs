// Bitcoin Dev Kit
// Written in 2021 by Rajarshi Maitra <rajarshi149@gmail.com>
//                    John Cantrell <johncantrell97@protonmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use std::fs::File;
use std::io::prelude::*;

use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::{
    mpsc::{channel, Receiver, SendError, Sender},
    Arc, RwLock,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use std::net::{SocketAddr, ToSocketAddrs};

use std::sync::PoisonError;
use std::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult};

use serde::{Deserialize, Serialize};
use serde_json::Error as SerdeError;

use super::{Mempool, Peer, PeerError};

use bitcoin::network::{
    constants::{Network, ServiceFlags},
    message::NetworkMessage,
    Address,
};

/// Default address pool minimums
const MIN_CBF_BUFFER: usize = 5;
const MIN_NONCBF_BUFFER: usize = 5;

/// A Discovery structure used by workers
///
/// Discovery can be initiated via a cache,
/// Or it will start with default hardcoded seeds
pub struct AddressDiscovery {
    pending: VecDeque<SocketAddr>,
    visited: HashSet<SocketAddr>,
}

impl AddressDiscovery {
    fn new(network: Network, seeds: VecDeque<SocketAddr>) -> AddressDiscovery {
        let mut network_seeds = AddressDiscovery::seeds(network);
        let mut total_seeds = seeds;
        total_seeds.append(&mut network_seeds);
        AddressDiscovery {
            pending: total_seeds,
            visited: HashSet::new(),
        }
    }

    fn add_pendings(&mut self, addresses: Vec<SocketAddr>) {
        for addr in addresses {
            if !self.pending.contains(&addr) && !self.visited.contains(&addr) {
                self.pending.push_back(addr);
            }
        }
    }

    fn get_next(&mut self) -> Option<SocketAddr> {
        match self.pending.pop_front() {
            None => None,
            Some(next) => {
                self.visited.insert(next);
                Some(next)
            }
        }
    }

    fn seeds(network: Network) -> VecDeque<SocketAddr> {
        let mut seeds = VecDeque::new();

        let port: u16 = match network {
            Network::Bitcoin => 8333,
            Network::Testnet => 18333,
            Network::Regtest => 18444,
            Network::Signet => 38333,
        };

        let seedhosts: &[&str] = match network {
            Network::Bitcoin => &[
                "seed.bitcoin.sipa.be",
                "dnsseed.bluematt.me",
                "dnsseed.bitcoin.dashjr.org",
                "seed.bitcoinstats.com",
                "seed.bitcoin.jonasschnelli.ch",
                "seed.btc.petertodd.org",
                "seed.bitcoin.sprovoost.nl",
                "dnsseed.emzy.de",
                "seed.bitcoin.wiz.biz",
            ],
            Network::Testnet => &[
                "testnet-seed.bitcoin.jonasschnelli.ch",
                "seed.tbtc.petertodd.org",
                "seed.testnet.bitcoin.sprovoost.nl",
                "testnet-seed.bluematt.me",
            ],
            Network::Regtest => &[],
            Network::Signet => &[],
        };

        for seedhost in seedhosts.iter() {
            if let Ok(lookup) = (*seedhost, port).to_socket_addrs() {
                for host in lookup {
                    if host.is_ipv4() {
                        seeds.push_back(host);
                    }
                }
            }
        }
        seeds
    }
}

/// Crawler structure that will interface with Discovery and public bitcoin network
///
/// Address manager will spawn multiple crawlers in separate threads to discover new addresses.
struct AddressWorker {
    discovery: Arc<RwLock<AddressDiscovery>>,
    sender: Sender<(SocketAddr, ServiceFlags)>,
    network: Network,
}

impl AddressWorker {
    fn new(
        discovery: Arc<RwLock<AddressDiscovery>>,
        sender: Sender<(SocketAddr, ServiceFlags)>,
        network: Network,
    ) -> AddressWorker {
        AddressWorker {
            discovery,
            sender,
            network,
        }
    }

    fn try_receive_addr(&mut self, peer: &Peer) -> Result<(), AddressManagerError> {
        if let Some(NetworkMessage::Addr(new_addresses)) =
            peer.recv("addr", Some(Duration::from_secs(1)))?
        {
            self.consume_addr(new_addresses)?;
        }

        Ok(())
    }

    fn consume_addr(&mut self, addrs: Vec<(u32, Address)>) -> Result<(), AddressManagerError> {
        let mut discovery_lock = self.discovery.write().map_err(PeerError::from)?;
        let mut addresses = Vec::new();
        for network_addrs in addrs {
            if let Ok(socket_addrs) = network_addrs.1.socket_addr() {
                addresses.push(socket_addrs);
            }
        }
        discovery_lock.add_pendings(addresses);

        Ok(())
    }

    fn work(&mut self) -> Result<(), AddressManagerError> {
        loop {
            let next_address = {
                let mut address_discovery = self.discovery.write()?;
                address_discovery.get_next()
            };

            match next_address {
                Some(address) => {
                    let potential_peer = Peer::connect_with_timeout(
                        address,
                        Duration::from_secs(1),
                        Arc::new(Mempool::default()),
                        self.network,
                    );

                    if let Ok(peer) = potential_peer {
                        peer.send(NetworkMessage::GetAddr)?;
                        self.try_receive_addr(&peer)?;
                        self.try_receive_addr(&peer)?;
                        self.sender.send((address, peer.get_version().services))?;
                        // TODO: Investigate why close is being called on non existent connections
                        // currently the errors are ignored
                        peer.close().unwrap_or(());
                    }
                }
                None => continue,
            }
        }
    }
}

/// A dedicated cache structure, with cbf/non_cbf separation
///
/// [AddressCache] will interface with file i/o
/// And can te turned into seeds. Generation of seed will put previously cached
/// cbf addresses at front of the vec, to boost up cbf node findings
#[derive(Serialize, Deserialize)]
struct AddressCache {
    banned_peers: HashSet<SocketAddr>,
    cbf: HashSet<SocketAddr>,
    non_cbf: HashSet<SocketAddr>,
}

impl AddressCache {
    fn empty() -> Self {
        Self {
            banned_peers: HashSet::new(),
            cbf: HashSet::new(),
            non_cbf: HashSet::new(),
        }
    }

    fn from_file(path: &str) -> Result<Option<Self>, AddressManagerError> {
        let serialized: Result<String, _> = std::fs::read_to_string(path);
        let serialized = match serialized {
            Ok(contents) => contents,
            Err(_) => return Ok(None),
        };

        let address_cache = serde_json::from_str(&serialized)?;
        Ok(Some(address_cache))
    }

    fn write_to_file(&self, path: &str) -> Result<(), AddressManagerError> {
        let serialized = serde_json::to_string_pretty(&self)?;

        let mut cache_file = File::create(path)?;

        cache_file.write_all(serialized.as_bytes())?;

        Ok(())
    }

    fn make_seeds(&self) -> VecDeque<SocketAddr> {
        self.cbf
            .iter()
            .chain(self.non_cbf.iter())
            .copied()
            .collect()
    }

    fn remove_address(&mut self, addrs: &SocketAddr, cbf: bool) -> bool {
        if cbf {
            self.cbf.remove(addrs)
        } else {
            self.non_cbf.remove(addrs)
        }
    }

    fn add_address(&mut self, addrs: SocketAddr, cbf: bool) -> bool {
        if cbf {
            self.cbf.insert(addrs)
        } else {
            self.non_cbf.insert(addrs)
        }
    }

    fn add_to_banlist(&mut self, addrs: SocketAddr, cbf: bool) {
        if self.banned_peers.insert(addrs) {
            self.remove_address(&addrs, cbf);
        }
    }
}

/// A Live directory maintained by [AddressManager] of freshly found cbf and non_cbf nodes by workers
///
/// Each instance of new [AddressManager] with have fresh [AddressDirectory]
/// This is independent from the cache and will be an in-memory database to
/// fetch addresses to the user.  
struct AddressDirectory {
    cbf_nodes: HashSet<SocketAddr>,
    non_cbf_nodes: HashSet<SocketAddr>,

    // List of addresses it has previously provided to the caller (PeerManager)
    previously_sent: HashSet<SocketAddr>,
}

impl AddressDirectory {
    fn new() -> AddressDirectory {
        AddressDirectory {
            cbf_nodes: HashSet::new(),
            non_cbf_nodes: HashSet::new(),
            previously_sent: HashSet::new(),
        }
    }

    fn add_address(&mut self, addr: SocketAddr, cbf: bool) {
        if cbf {
            self.cbf_nodes.insert(addr);
        } else {
            self.non_cbf_nodes.insert(addr);
        }
    }

    fn get_new_address(&mut self, cbf: bool) -> Option<SocketAddr> {
        if cbf {
            if let Some(new_addresses) = self
                .cbf_nodes
                .iter()
                .filter(|item| !self.previously_sent.contains(item))
                .collect::<Vec<&SocketAddr>>()
                .pop()
            {
                self.previously_sent.insert(*new_addresses);
                Some(*new_addresses)
            } else {
                None
            }
        } else if let Some(new_addresses) = self
            .non_cbf_nodes
            .iter()
            .filter(|item| !self.previously_sent.contains(item))
            .collect::<Vec<&SocketAddr>>()
            .pop()
        {
            self.previously_sent.insert(*new_addresses);
            Some(*new_addresses)
        } else {
            None
        }
    }

    fn get_cbf_address_count(&self) -> usize {
        self.cbf_nodes.len()
    }

    fn get_non_cbf_address_count(&self) -> usize {
        self.non_cbf_nodes.len()
    }

    fn remove_address(&mut self, addrs: &SocketAddr, cbf: bool) {
        if cbf {
            self.cbf_nodes.remove(addrs);
        } else {
            self.non_cbf_nodes.remove(addrs);
        }
    }

    fn get_cbf_buffer(&self) -> usize {
        self.cbf_nodes
            .iter()
            .filter(|item| !self.previously_sent.contains(item))
            .count()
    }

    fn get_non_cbf_buffer(&self) -> usize {
        self.non_cbf_nodes
            .iter()
            .filter(|item| !self.previously_sent.contains(item))
            .count()
    }
}

/// Discovery statistics, useful for logging
#[derive(Clone, Copy)]
pub struct DiscoveryData {
    queued: usize,
    visited: usize,
    non_cbf_count: usize,
    cbf_count: usize,
}

/// Progress trait for discovery statistics logging
pub trait DiscoveryProgress {
    /// Update progress
    fn update(&self, data: DiscoveryData);
}

/// Used when progress updates are not desired
#[derive(Clone)]
pub struct NoDiscoveryProgress;

impl DiscoveryProgress for NoDiscoveryProgress {
    fn update(&self, _data: DiscoveryData) {}
}

/// Used to log progress update
#[derive(Clone)]
pub struct LogDiscoveryProgress;

impl DiscoveryProgress for LogDiscoveryProgress {
    fn update(&self, data: DiscoveryData) {
        log::trace!(
            "P2P Discovery: {} queued, {} visited, {} connected, {} cbf_enabled",
            data.queued,
            data.visited,
            data.non_cbf_count,
            data.cbf_count
        );

        #[cfg(test)]
        println!(
            "P2P Discovery: {} queued, {} visited, {} connected, {} cbf_enabled",
            data.queued, data.visited, data.non_cbf_count, data.cbf_count
        );
    }
}

/// A manager structure managing address discovery
///
/// Manager will try to maintain a given address buffer in its directory
/// buffer = len(exiting addresses) - len(previously provided addresses)
/// Manager will crawl the network until buffer criteria is satisfied
/// Manager will bootstrap workers from a cache, to speed up discovery progress in
/// subsequent call after the first crawl.
/// Manager will keep track of the cache and only update it if previously
/// unknown addresses are found.
pub struct AddressManager<P: DiscoveryProgress> {
    directory: AddressDirectory,
    cache_filename: String,
    discovery: Arc<RwLock<AddressDiscovery>>,
    threads: usize,
    receiver: Receiver<(SocketAddr, ServiceFlags)>,
    sender: Sender<(SocketAddr, ServiceFlags)>,
    network: Network,
    cbf_buffer: usize,
    non_cbf_buffer: usize,
    progress: P,
}

impl<P: DiscoveryProgress> AddressManager<P> {
    /// Create a new manager. Initiate Discovery seeds from the cache
    /// if it exists, else start with hardcoded seeds
    pub fn new(
        network: Network,
        cache_filename: String,
        threads: usize,
        cbf_buffer: Option<usize>,
        non_cbf_buffer: Option<usize>,
        progress: P,
    ) -> Result<AddressManager<P>, AddressManagerError> {
        let (sender, receiver) = channel();

        let seeds = match AddressCache::from_file(&cache_filename)? {
            Some(cache) => cache.make_seeds(),
            None => VecDeque::new(),
        };

        let min_cbf = cbf_buffer.unwrap_or(MIN_CBF_BUFFER);

        let min_non_cbf = non_cbf_buffer.unwrap_or(MIN_NONCBF_BUFFER);

        let discovery = AddressDiscovery::new(network, seeds);

        Ok(AddressManager {
            cache_filename,
            directory: AddressDirectory::new(),
            discovery: Arc::new(RwLock::new(discovery)),
            sender,
            receiver,
            network,
            threads,
            cbf_buffer: min_cbf,
            non_cbf_buffer: min_non_cbf,
            progress,
        })
    }

    /// Get running address discovery progress
    fn get_progress(&self) -> Result<DiscoveryData, AddressManagerError> {
        let (queued_count, visited_count) = {
            let address_discovery = self.discovery.read()?;
            (
                address_discovery.pending.len(),
                address_discovery.visited.len(),
            )
        };

        let cbf_node_count = self.directory.get_cbf_address_count();
        let other_node_count = self.directory.get_non_cbf_address_count();

        Ok(DiscoveryData {
            queued: queued_count,
            visited: visited_count,
            non_cbf_count: cbf_node_count + other_node_count,
            cbf_count: cbf_node_count,
        })
    }

    /// Spawn [self.thread] no. of worker threads
    fn spawn_workers(&mut self) -> Vec<JoinHandle<()>> {
        let mut worker_handles: Vec<JoinHandle<()>> = vec![];
        for _ in 0..self.threads {
            let sender = self.sender.clone();
            let discovery = self.discovery.clone();
            let network = self.network;
            let worker_handle = thread::spawn(move || {
                let mut worker = AddressWorker::new(discovery, sender, network);
                worker.work().unwrap();
            });
            worker_handles.push(worker_handle);
        }
        worker_handles
    }

    /// Crawl the Bitcoin network until required number of cbf/non_cbf nodes are found
    ///
    /// - This will start a bunch of crawlers.
    /// - load up the existing cache.
    /// - Update the cache with new found peers.
    /// - check if address is in banlist
    /// - run crawlers until buffer requirement is matched
    /// - flush the current cache into disk
    pub fn fetch(&mut self) -> Result<(), AddressManagerError> {
        self.spawn_workers();

        // Get already existing cache
        let mut cache = match AddressCache::from_file(&self.cache_filename)? {
            Some(cache) => cache,
            None => AddressCache::empty(),
        };

        while self.directory.get_cbf_buffer() < self.cbf_buffer
            || self.directory.get_non_cbf_buffer() < self.non_cbf_buffer
        {
            if let Ok(message) = self.receiver.recv() {
                let (addr, flag) = message;
                if !cache.banned_peers.contains(&addr) {
                    let cbf = flag.has(ServiceFlags::COMPACT_FILTERS);
                    self.directory.add_address(addr, cbf);
                    cache.add_address(addr, cbf);
                }
            }
        }

        self.progress.update(self.get_progress()?);

        // When completed, flush the cache
        cache.write_to_file(&self.cache_filename)?;

        Ok(())
    }

    /// Get a new addresses not previously provided
    pub fn get_new_cbf_address(&mut self) -> Option<SocketAddr> {
        self.directory.get_new_address(true)
    }

    /// Get a new non_cbf address
    pub fn get_new_non_cbf_address(&mut self) -> Option<SocketAddr> {
        self.directory.get_new_address(false)
    }

    /// Ban an address
    pub fn ban_peer(&mut self, addrs: &SocketAddr, cbf: bool) -> Result<(), AddressManagerError> {
        let mut cache = AddressCache::from_file(&self.cache_filename)?.ok_or_else(|| {
            AddressManagerError::Generic("Address Cache file not found".to_string())
        })?;

        cache.add_to_banlist(*addrs, cbf);

        // When completed, flush the cache
        cache.write_to_file(&self.cache_filename).unwrap();

        self.directory.remove_address(addrs, cbf);

        Ok(())
    }

    /// Get all the known CBF addresses
    pub fn get_known_cbfs(&self) -> Option<Vec<SocketAddr>> {
        let addresses = self
            .directory
            .cbf_nodes
            .iter()
            .copied()
            .collect::<Vec<SocketAddr>>();

        match addresses.len() {
            0 => None,
            _ => Some(addresses),
        }
    }

    /// Get all the known regular addresses
    pub fn get_known_non_cbfs(&self) -> Option<Vec<SocketAddr>> {
        let addresses = self
            .directory
            .non_cbf_nodes
            .iter()
            .copied()
            .collect::<Vec<SocketAddr>>();

        match addresses.len() {
            0 => None,
            _ => Some(addresses),
        }
    }

    /// Get previously tried addresses
    pub fn get_previously_tried(&self) -> Option<Vec<SocketAddr>> {
        let addresses = self
            .directory
            .previously_sent
            .iter()
            .copied()
            .collect::<Vec<SocketAddr>>();

        match addresses.len() {
            0 => None,
            _ => Some(addresses),
        }
    }
}

#[derive(Debug)]
pub enum AddressManagerError {
    /// Std I/O Error
    Io(std::io::Error),

    /// Internal Peer error
    Peer(PeerError),

    /// Internal Mutex poisoning error
    MutexPoisoned,

    /// Internal Mutex wait timed out
    MutexTimedOut,

    /// Internal RW read lock poisoned
    RwReadLockPoisined,

    /// Internal RW write lock poisoned
    RwWriteLockPoisoned,

    /// Internal MPSC sending error
    MpscSendError,

    /// Serde Json Error
    SerdeJson(SerdeError),

    /// Generic Errors
    Generic(String),
}

impl_error!(PeerError, Peer, AddressManagerError);
impl_error!(std::io::Error, Io, AddressManagerError);
impl_error!(SerdeError, SerdeJson, AddressManagerError);

impl<T> From<PoisonError<MutexGuard<'_, T>>> for AddressManagerError {
    fn from(_: PoisonError<MutexGuard<'_, T>>) -> Self {
        AddressManagerError::MutexPoisoned
    }
}

impl<T> From<PoisonError<RwLockWriteGuard<'_, T>>> for AddressManagerError {
    fn from(_: PoisonError<RwLockWriteGuard<'_, T>>) -> Self {
        AddressManagerError::RwWriteLockPoisoned
    }
}

impl<T> From<PoisonError<RwLockReadGuard<'_, T>>> for AddressManagerError {
    fn from(_: PoisonError<RwLockReadGuard<'_, T>>) -> Self {
        AddressManagerError::RwReadLockPoisined
    }
}

impl<T> From<PoisonError<(MutexGuard<'_, T>, WaitTimeoutResult)>> for AddressManagerError {
    fn from(err: PoisonError<(MutexGuard<'_, T>, WaitTimeoutResult)>) -> Self {
        let (_, wait_result) = err.into_inner();
        if wait_result.timed_out() {
            AddressManagerError::MutexTimedOut
        } else {
            AddressManagerError::MutexPoisoned
        }
    }
}

impl<T> From<SendError<T>> for AddressManagerError {
    fn from(_: SendError<T>) -> Self {
        AddressManagerError::MpscSendError
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_crawl_times() {
        // Initiate a manager with an non existent cache file name.
        // It will create a new cache file
        let mut manager = AddressManager::new(
            Network::Bitcoin,
            "addr_cache".to_string(),
            20,
            None,
            None,
            LogDiscoveryProgress,
        )
        .unwrap();

        // start the crawlers and time them
        let start = std::time::Instant::now();
        manager.fetch().unwrap();
        let duration1 = start.elapsed();

        // Create a new manager from existing cache and fetch again
        let mut manager = AddressManager::new(
            Network::Bitcoin,
            "addr_cache".to_string(),
            20,
            None,
            None,
            LogDiscoveryProgress,
        )
        .unwrap();

        // start the crawlers and time them
        let start = std::time::Instant::now();
        manager.fetch().unwrap();
        let duration2 = start.elapsed();

        println!("Time taken for initial crawl: {:#?}", duration1);
        println!("Time taken for next crawl {:#?}", duration2);
    }

    #[test]
    fn test_buffer_management() {
        // Initiate a manager with an non existent cache file name.
        // It will create a new cache file
        let mut manager = AddressManager::new(
            Network::Bitcoin,
            "addr_cache".to_string(),
            20,
            None,
            None,
            LogDiscoveryProgress,
        )
        .unwrap();

        // Start the first fetch()
        manager.fetch().unwrap();

        // Fetch few new address and ensure buffer goes to zero
        let mut addrs_list = Vec::new();
        for _ in 0..5 {
            let addr_cbf = manager.get_new_cbf_address().unwrap();
            let addrs_non_cbf = manager.get_new_non_cbf_address().unwrap();

            addrs_list.push(addr_cbf);

            addrs_list.push(addrs_non_cbf);
        }

        assert_eq!(addrs_list.len(), 10);

        // This should exhaust the cbf buffer
        assert_eq!(manager.directory.get_cbf_buffer(), 0);

        // Calling fetch again should start crawlers until buffer
        // requirements are matched.
        manager.fetch().unwrap();

        // It should again have a cbf buffer of 5
        assert_eq!(manager.directory.get_cbf_buffer(), 5);
    }
}
