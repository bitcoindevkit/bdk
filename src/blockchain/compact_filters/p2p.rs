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

use std::io::prelude::*;
use std::fs::File;
use std::path::Path;

use std::collections::HashSet;
use std::collections::VecDeque;

use std::time::Duration;
use std::sync::{Arc, Mutex, mpsc::{channel, Sender, Receiver}};
use std::thread::{self, JoinHandle};
use std::convert::From;

use std::net::{SocketAddr, ToSocketAddrs};

use serde::{Deserialize, Serialize};

use super::{Peer, Mempool};
use crate::{Error};

use bitcoin::network::{
    constants::{ServiceFlags, Network},
    message::{NetworkMessage},
    Address
};

pub struct AddressDiscovery {
    pending: VecDeque<SocketAddr>,
    visited: HashSet<SocketAddr>
}

impl AddressDiscovery {
    pub fn new(network: Network, mut seeds: VecDeque<SocketAddr>) -> AddressDiscovery {
        let mut network_seeds = AddressDiscovery::seeds(network);
        seeds.append(&mut network_seeds);
        AddressDiscovery {
            pending: seeds,
            visited: HashSet::new()
        }
    }

    pub fn add_pending(&mut self, addr: SocketAddr) {
        if !self.pending.contains(&addr) && !self.visited.contains(&addr) {
            self.pending.push_back(addr);
        }
    }

    pub fn get_next(&mut self) -> Option<SocketAddr> {
        match self.pending.pop_front() {
            None => { None },
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
            Network::Signet => 38333
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
            Network::Signet => &[]
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

pub struct AddressWorker  {
    discovery: Arc<Mutex<AddressDiscovery>>,
    sender: Sender<AddressMessage>,
    network: Network
}

impl AddressWorker {
    pub fn new(discovery: Arc<Mutex<AddressDiscovery>>, sender: Sender<AddressMessage>, network: Network) -> AddressWorker {
        AddressWorker { discovery, sender, network }
    }

    fn try_receive_addr(&mut self, peer: &Peer) {
        match peer.recv("addr", Some(Duration::from_secs(1))).unwrap() {
            Some(NetworkMessage::Addr(new_addresses)) => {
                self.consume_addr(new_addresses);
            },
            _ => {}
        }
    }

    fn consume_addr(&mut self, addrs: Vec<(u32, Address)>) {
        for (_, addr) in addrs.iter() {
            if let Ok(socket_addr) = addr.socket_addr() {
                self.sender.send(AddressMessage::NewPending(socket_addr));
            }
        }
    }

    pub fn work(&mut self) {
        loop {
            let next_address = {
                let mut address_discovery = self.discovery.lock().unwrap();
                address_discovery.get_next()
            };

            match next_address {
                Some(address) => {
                    let potential_peer = Peer::connect_with_timeout(address, Duration::from_secs(3), Arc::new(Mempool::default()), self.network);

                    if let Ok(peer) = potential_peer {
                        peer.send(NetworkMessage::GetAddr).unwrap();
                        self.try_receive_addr(&peer);
                        self.try_receive_addr(&peer);
                        self.sender.send(AddressMessage::NewValid(address, peer.get_version().services));
                        peer.close();
                    }
                },
                None => {
                    break
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct AddressCache {
    addresses: VecDeque<SocketAddr>,
}

impl From<&AddressDirectory> for AddressCache {
    fn from(directory: &AddressDirectory) -> Self {
        AddressCache {
            addresses: directory.get_addrs()
        }
    }
}

impl AddressCache {
    pub fn from_file(path: &str) -> Option<AddressCache> {
        let serialized: Result<String, _ > = std::fs::read_to_string(path);
        let serialized = match serialized {
            Ok(contents) => contents,
            Err(_) => { return None }
        };
        
        let address_cache: AddressCache = serde_json::from_str(&serialized).unwrap();
        Some(address_cache)
    }
}

pub struct NodeAddress {
    addr: SocketAddr,
    service_flag: ServiceFlags
}

pub struct AddressDirectory {
    cbf_enabled_nodes: VecDeque<NodeAddress>,
    other_nodes: VecDeque<NodeAddress>,
    cache_filename: String
}

impl AddressDirectory {
    pub fn new(cache_filename: String) -> AddressDirectory {
        AddressDirectory {
            cbf_enabled_nodes: VecDeque::new(),
            other_nodes: VecDeque::new(),
            cache_filename
        }
    }
    
    pub fn get_addrs(&self) -> VecDeque<SocketAddr> {
        let mut other_nodes = self.other_nodes
            .iter()
            .map(|na| { na.addr })
            .collect::<VecDeque<SocketAddr>>();

        let mut cbf_nodes = self.cbf_enabled_nodes
            .iter()
            .map(|na| { na.addr })
            .collect::<VecDeque<SocketAddr>>();

        cbf_nodes.append(&mut other_nodes);
        cbf_nodes
    }

    pub fn add_node(&mut self, addr: SocketAddr, flag: ServiceFlags) {
        let node_address = NodeAddress { addr, service_flag: flag };

        if flag.has(ServiceFlags::COMPACT_FILTERS) {
            self.cbf_enabled_nodes.push_back(node_address);
            self.cache_to_fs();
        } else {
            self.other_nodes.push_back(node_address);
        }
    }

    pub fn get_cbf_enabled_node(&mut self) -> Option<NodeAddress> {
        self.cbf_enabled_nodes.pop_front()
    }

    pub fn get_other_node(&mut self) -> Option<NodeAddress> {
        self.other_nodes.pop_front()
    }

    pub fn cache_to_fs(&self) {
        let cache = AddressCache::from(self);
        let serialized = serde_json::to_string(&cache).unwrap();
        let path = Path::new(&self.cache_filename);
        let mut file = File::create(&path).unwrap();
        file.write_all(serialized.as_bytes()).unwrap();
    }
}


pub enum AddressMessage {
    NewPending(SocketAddr),
    NewValid(SocketAddr, ServiceFlags)
}

#[derive(Clone, Copy)]
pub struct DiscoveryData {
    pub queued: usize,
    pub visited: usize,
    pub connected: usize,
    pub cbf_enabled: usize
}

pub trait DiscoveryProgress {
    fn update(&self, data: DiscoveryData) -> Result<(), Error>;
}

pub fn discovery_progress() -> (Sender<DiscoveryData>, Receiver<DiscoveryData>) {
    channel()
}

impl DiscoveryProgress for Sender<DiscoveryData> {
    fn update(&self, data: DiscoveryData) -> Result<(), Error> {
        self.send(data)
            .map_err(|_| Error::ProgressUpdateError)
    }
}

#[derive(Clone)]
pub struct NoopDiscoveryProgress;

pub fn noop_discovery_progress() -> NoopDiscoveryProgress {
    NoopDiscoveryProgress
}

impl DiscoveryProgress for NoopDiscoveryProgress {
    fn update(&self, _data: DiscoveryData) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct LogDiscoveryProgress;

pub fn log_discovery_progress() -> LogDiscoveryProgress {
    LogDiscoveryProgress
}

impl DiscoveryProgress for LogDiscoveryProgress {
    fn update(&self, data: DiscoveryData) -> Result<(), Error> {
        log::trace!(
            "P2P Discovery: {} queued, {} visited, {} connected, {} cbf_enabled",
            data.queued, data.visited, data.connected, data.cbf_enabled
        );
        Ok(())
    }
}

pub struct AddressManager {
    directory: AddressDirectory,
    cache_filename: String,
    discovery: Arc<Mutex<AddressDiscovery>>,
    threads: usize,
    receiver: Receiver<AddressMessage>,
    sender: Sender<AddressMessage>,
    network: Network
}

impl AddressManager {
    pub fn new(network: Network, cache_filename: String, threads: usize) -> AddressManager {
        let (sender, receiver) = channel();

        let filename = cache_filename.clone();
        let seeds = match AddressCache::from_file(&cache_filename) {
            Some(cache) => { cache.addresses },
            None => { VecDeque::new() }
        };

        AddressManager {
            cache_filename,
            directory: AddressDirectory::new(filename),
            discovery: Arc::new(Mutex::new(AddressDiscovery::new(network, seeds))),
            sender,
            receiver,
            network,
            threads
        }
    }

    pub fn get_worker(&self) -> AddressWorker {
        let sender = self.sender.clone();
        let discovery = self.discovery.clone();
        let network = self.network.clone();

        AddressWorker::new(discovery, sender, network)
    }

    pub fn get_progress(&mut self) -> DiscoveryData {
        let (queued_count, visited_count) = {
        let address_discovery = self.discovery.lock().unwrap();
            (address_discovery.pending.len(), address_discovery.visited.len())
        };
        
        let cbf_node_count = self.directory.cbf_enabled_nodes.len();
        let other_node_count = self.directory.other_nodes.len();

        DiscoveryData {
            queued: queued_count,
            visited: visited_count,
            connected: cbf_node_count + other_node_count,
            cbf_enabled: cbf_node_count
        }
    }

    pub fn discover(&mut self) -> Vec<JoinHandle<()>> {
        let mut worker_handles: Vec<JoinHandle<()>> = vec![];
        for _i in 1..self.threads {
            let sender = self.sender.clone();
            let discovery = self.discovery.clone();
            let network = self.network.clone();
            let worker_handle = thread::spawn(move || {
                let mut worker = AddressWorker::new(discovery, sender, network);
                 worker.work();
            });
            worker_handles.push(worker_handle);
        }
        worker_handles
    }

    pub fn manage<P: 'static + DiscoveryProgress>(&mut self, progress_update: P) {
        let _worker_handles = self.discover();

        loop {
            if let Ok(message) = self.receiver.recv() {
                match message {
                    AddressMessage::NewPending(addr) => {
                        if addr.is_ipv4() {
                            let mut address_discovery = self.discovery.lock().unwrap();
                            address_discovery.add_pending(addr);
                        }
                    },
                    AddressMessage::NewValid(addr,flag) => {
                        self.directory.add_node(addr, flag);
                    }
                }

                // TODO: how to handle update failure
                progress_update.update(self.get_progress()).unwrap();
            }
        }
    }
}
