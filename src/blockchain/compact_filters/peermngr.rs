use super::address_manager::{AddressManager, AddressManagerError, DiscoveryProgress};
use super::peer::{Mempool, Peer, PeerError, TIMEOUT_SECS};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time;

use bitcoin::network::constants::{Network, ServiceFlags};
use bitcoin::network::message::NetworkMessage;

use std::path::PathBuf;

use std::collections::BTreeMap;

// Peer Manager Configuration constants
const MIN_CBF_PEERS: usize = 2;
const MIN_TOTAL_PEERS: usize = 5;
const MIN_CRAWLER_THREADS: usize = 20;
const BAN_SCORE_THRESHOLD: usize = 100;
const RECEIVE_TIMEOUT: time::Duration = time::Duration::from_secs(TIMEOUT_SECS);

#[allow(dead_code)]
/// An Error structure describing Peer Management errors
#[derive(Debug)]
pub enum PeerManagerError {
    // Internal Peer Error
    Peer(PeerError),

    // Internal AddressManager Error
    AddrsManager(AddressManagerError),

    // Os String Error
    OsString(std::ffi::OsString),

    // Peer not found in directory
    PeerNotFound,

    // Generic Internal Error
    Generic(String),
}

impl_error!(PeerError, Peer, PeerManagerError);
impl_error!(AddressManagerError, AddrsManager, PeerManagerError);
impl_error!(std::ffi::OsString, OsString, PeerManagerError);

/// Peer Data stored in the manager's directory
#[derive(Debug)]
struct PeerData {
    peer: Peer,
    is_cbf: bool,
    ban_score: usize,
}

#[allow(dead_code)]
/// A Directory structure to hold live Peers
/// All peers in the directory have live ongoing connection
/// Banning a peer removes it from the directory
#[derive(Default, Debug)]
struct PeerDirectory {
    peers: BTreeMap<SocketAddr, PeerData>,
}

#[allow(dead_code)]
impl PeerDirectory {
    fn new() -> Self {
        Self::default()
    }

    fn get_cbf_peers(&self) -> Option<Vec<&PeerData>> {
        let cbf_peers = self
            .peers
            .iter()
            .filter(|(_, peer)| peer.is_cbf)
            .map(|(_, peer)| peer)
            .collect::<Vec<&PeerData>>();

        match cbf_peers.len() {
            0 => None,
            _ => Some(cbf_peers),
        }
    }

    fn get_cbf_addresses(&self) -> Option<Vec<SocketAddr>> {
        let cbf_addrseses = self
            .peers
            .iter()
            .filter_map(
                |(addrs, peerdata)| {
                    if peerdata.is_cbf {
                        Some(addrs)
                    } else {
                        None
                    }
                },
            )
            .copied()
            .collect::<Vec<SocketAddr>>();

        match cbf_addrseses.len() {
            0 => None,
            _ => Some(cbf_addrseses),
        }
    }

    fn get_non_cbf_peers(&self) -> Option<Vec<&PeerData>> {
        let non_cbf_peers = self
            .peers
            .iter()
            .filter(|(_, peerdata)| !peerdata.is_cbf)
            .map(|(_, peerdata)| peerdata)
            .collect::<Vec<&PeerData>>();

        match non_cbf_peers.len() {
            0 => None,
            _ => Some(non_cbf_peers),
        }
    }

    fn get_non_cbf_addresses(&self) -> Option<Vec<SocketAddr>> {
        let addresses = self
            .peers
            .iter()
            .filter_map(
                |(addrs, peerdata)| {
                    if !peerdata.is_cbf {
                        Some(addrs)
                    } else {
                        None
                    }
                },
            )
            .copied()
            .collect::<Vec<SocketAddr>>();

        match addresses.len() {
            0 => None,
            _ => Some(addresses),
        }
    }

    fn get_cbf_peers_mut(&mut self) -> Option<Vec<&mut PeerData>> {
        let peers = self
            .peers
            .iter_mut()
            .filter(|(_, peerdata)| peerdata.is_cbf)
            .map(|(_, peerdata)| peerdata)
            .collect::<Vec<&mut PeerData>>();

        match peers.len() {
            0 => None,
            _ => Some(peers),
        }
    }

    fn get_non_cbf_peers_mut(&mut self) -> Option<Vec<&mut PeerData>> {
        let peers = self
            .peers
            .iter_mut()
            .filter(|(_, peerdata)| !peerdata.is_cbf)
            .map(|(_, peerdata)| peerdata)
            .collect::<Vec<&mut PeerData>>();

        match peers.len() {
            0 => None,
            _ => Some(peers),
        }
    }

    fn get_cbf_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|(_, peerdata)| peerdata.is_cbf)
            .count()
    }

    fn get_non_cbf_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|(_, peerdata)| !peerdata.is_cbf)
            .count()
    }

    fn insert_peer(&mut self, peerdata: PeerData) -> Result<(), PeerManagerError> {
        let addrs = peerdata.peer.get_address()?;
        self.peers.entry(addrs).or_insert(peerdata);
        Ok(())
    }

    fn remove_peer(&mut self, addrs: &SocketAddr) -> Option<PeerData> {
        self.peers.remove(addrs)
    }

    fn get_peer_banscore(&self, addrs: &SocketAddr) -> Option<usize> {
        self.peers.get(addrs).map(|peerdata| peerdata.ban_score)
    }

    fn get_peerdata_mut(&mut self, address: &SocketAddr) -> Option<&mut PeerData> {
        self.peers.get_mut(address)
    }

    fn get_peerdata(&self, address: &SocketAddr) -> Option<&PeerData> {
        self.peers.get(address)
    }

    fn is_cbf(&self, addrs: &SocketAddr) -> Option<bool> {
        if let Some(peer) = self.peers.get(addrs) {
            match peer.is_cbf {
                true => Some(true),
                false => Some(false),
            }
        } else {
            None
        }
    }
}

#[allow(dead_code)]
pub struct PeerManager<P: DiscoveryProgress> {
    addrs_mngr: AddressManager<P>,
    directory: PeerDirectory,
    mempool: Arc<Mempool>,
    min_cbf: usize,
    min_total: usize,
    network: Network,
}

#[allow(dead_code)]
impl<P: DiscoveryProgress> PeerManager<P> {
    pub fn init(
        network: Network,
        cache_dir: &str,
        crawler_threads: Option<usize>,
        progress: P,
        cbf_peers: Option<usize>,
        total_peers: Option<usize>,
    ) -> Result<Self, PeerManagerError> {
        let mut cache_filename = PathBuf::from(cache_dir);
        cache_filename.push("addr_cache");

        // Fetch minimum peer requirements, either by user input, or via default
        let min_cbf = cbf_peers.unwrap_or(MIN_CBF_PEERS);

        let min_total = total_peers.unwrap_or(MIN_TOTAL_PEERS);

        let cbf_buff = min_cbf * 2;
        let non_cbf_buff = (min_total - min_cbf) * 2;

        // Create internal items
        let addrs_mngr = AddressManager::new(
            network,
            cache_filename.into_os_string().into_string()?,
            crawler_threads.unwrap_or(MIN_CRAWLER_THREADS),
            Some(cbf_buff),
            Some(non_cbf_buff),
            progress,
        )?;

        let mempool = Arc::new(Mempool::new());

        let peer_dir = PeerDirectory::new();

        // Create self and update
        let mut manager = Self {
            addrs_mngr,
            directory: peer_dir,
            mempool,
            min_cbf,
            min_total,
            network,
        };

        manager.update_directory()?;

        Ok(manager)
    }

    fn update_directory(&mut self) -> Result<(), PeerManagerError> {
        while self.directory.get_cbf_count() < self.min_cbf
            || self.directory.get_non_cbf_count() < (self.min_total - self.min_cbf)
        {
            // First connect with cbf peers, then with non_cbf
            let cbf_fetch = self.directory.get_cbf_count() < self.min_cbf;

            // Try to get an address
            // if not present start crawlers
            let target_addrs = match cbf_fetch {
                true => {
                    if let Some(addrs) = self.addrs_mngr.get_new_cbf_address() {
                        addrs
                    } else {
                        self.addrs_mngr.fetch()?;
                        continue;
                    }
                }
                false => {
                    if let Some(addrs) = self.addrs_mngr.get_new_non_cbf_address() {
                        addrs
                    } else {
                        self.addrs_mngr.fetch()?;
                        continue;
                    }
                }
            };

            if let Ok(peer) = Peer::connect(target_addrs, Arc::clone(&self.mempool), self.network) {
                let address = peer.get_address()?;

                assert_eq!(address, target_addrs);

                let is_cbf = peer
                    .get_version()
                    .services
                    .has(ServiceFlags::COMPACT_FILTERS);

                let peerdata = PeerData {
                    peer,
                    is_cbf,
                    ban_score: 0,
                };

                self.directory.insert_peer(peerdata)?;
            } else {
                continue;
            }
        }

        Ok(())
    }

    pub fn set_banscore(
        &mut self,
        increase_by: usize,
        address: &SocketAddr,
    ) -> Result<(), PeerManagerError> {
        let mut current_score = if let Some(peer) = self.directory.get_peerdata_mut(address) {
            peer.ban_score
        } else {
            return Err(PeerManagerError::PeerNotFound);
        };

        current_score += increase_by;

        let mut banned = false;

        if current_score >= BAN_SCORE_THRESHOLD {
            match (
                self.directory.is_cbf(address),
                self.directory.remove_peer(address),
            ) {
                (Some(true), Some(_)) => {
                    self.addrs_mngr.ban_peer(address, true)?;
                    banned = true;
                }
                (Some(false), Some(_)) => {
                    self.addrs_mngr.ban_peer(address, false)?;
                    banned = true;
                }
                _ => {
                    return Err(PeerManagerError::Generic(
                        "data inconsistency in directory, should not happen".to_string(),
                    ))
                }
            }
        }

        if banned {
            self.update_directory()?;
        }

        Ok(())
    }

    pub fn send_to(
        &self,
        address: &SocketAddr,
        message: NetworkMessage,
    ) -> Result<(), PeerManagerError> {
        if let Some(peerdata) = self.directory.get_peerdata(address) {
            peerdata.peer.send(message)?;
            Ok(())
        } else {
            Err(PeerManagerError::PeerNotFound)
        }
    }

    pub fn receive_from(
        &self,
        address: &SocketAddr,
        wait_for: &'static str,
    ) -> Result<Option<NetworkMessage>, PeerManagerError> {
        if let Some(peerdata) = self.directory.get_peerdata(address) {
            if let Some(response) = peerdata.peer.recv(wait_for, Some(RECEIVE_TIMEOUT))? {
                Ok(Some(response))
            } else {
                Ok(None)
            }
        } else {
            Err(PeerManagerError::PeerNotFound)
        }
    }

    pub fn connected_cbf_addresses(&self) -> Option<Vec<SocketAddr>> {
        self.directory.get_cbf_addresses()
    }

    pub fn connected_non_cbf_addresses(&self) -> Option<Vec<SocketAddr>> {
        self.directory.get_non_cbf_addresses()
    }

    pub fn known_cbf_addresses(&self) -> Option<Vec<SocketAddr>> {
        self.addrs_mngr.get_known_cbfs()
    }

    pub fn known_non_cbf_addresses(&self) -> Option<Vec<SocketAddr>> {
        self.addrs_mngr.get_known_non_cbfs()
    }

    pub fn previously_tried_addresses(&self) -> Option<Vec<SocketAddr>> {
        self.addrs_mngr.get_previously_tried()
    }
}

#[cfg(test)]
mod test {
    use super::super::LogDiscoveryProgress;
    use super::*;

    #[test]
    #[ignore]
    fn test_ban() {
        let mut manager = PeerManager::init(
            Network::Bitcoin,
            ".",
            None,
            LogDiscoveryProgress,
            None,
            None,
        )
        .unwrap();

        let connected_cbfs = manager.connected_cbf_addresses().unwrap();
        let connected_non_cbfs = manager.connected_non_cbf_addresses().unwrap();

        println!("Currently Connected CBFs: {:#?}", connected_cbfs);
        assert_eq!(connected_cbfs.len(), 2);
        assert_eq!(connected_non_cbfs.len(), 3);

        let to_banned = &connected_cbfs[0];

        println!("Banning address : {}", to_banned);

        manager.set_banscore(100, to_banned).unwrap();

        let newly_connected = manager.connected_cbf_addresses().unwrap();

        println!("Newly Connected CBFs: {:#?}", newly_connected);

        assert_eq!(newly_connected.len(), 2);

        assert_ne!(newly_connected, connected_cbfs);
    }

    #[test]
    #[ignore]
    fn test_send_recv() {
        let manager = PeerManager::init(
            Network::Bitcoin,
            ".",
            None,
            LogDiscoveryProgress,
            None,
            None,
        )
        .unwrap();

        let target_address = manager.connected_cbf_addresses().unwrap()[0];

        let ping = NetworkMessage::Ping(30);

        println!("Asking peer {}", target_address);

        manager.send_to(&target_address, ping).unwrap();

        let response = manager
            .receive_from(&target_address, "pong")
            .unwrap()
            .unwrap();

        let value = match response {
            NetworkMessage::Pong(v) => Some(v),
            _ => None,
        };

        let value = value.unwrap();

        println!("Got value {:#?}", value);
    }

    #[test]
    #[ignore]
    fn test_connect_all() {
        let manager = PeerManager::init(
            Network::Bitcoin,
            ".",
            None,
            LogDiscoveryProgress,
            None,
            None,
        )
        .unwrap();

        let cbf_pings = vec![100u64; manager.min_cbf];
        let non_cbf_pings = vec![200u64; manager.min_total - manager.min_cbf];

        let cbf_peers = manager.connected_cbf_addresses().unwrap();
        let non_cbf_peers = manager.connected_non_cbf_addresses().unwrap();

        let sent_cbf: Vec<bool> = cbf_pings
            .iter()
            .zip(cbf_peers.iter())
            .map(|(ping, address)| {
                let message = NetworkMessage::Ping(*ping);
                manager.send_to(address, message).unwrap();
                true
            })
            .collect();

        assert_eq!(sent_cbf, vec![true; manager.min_cbf]);

        println!("Sent pings to cbf peers");

        let sent_noncbf: Vec<bool> = non_cbf_pings
            .iter()
            .zip(non_cbf_peers.iter())
            .map(|(ping, address)| {
                let message = NetworkMessage::Ping(*ping);
                manager.send_to(address, message).unwrap();
                true
            })
            .collect();

        assert_eq!(sent_noncbf, vec![true; manager.min_total - manager.min_cbf]);

        println!("Sent pings to non cbf peers");

        let cbf_received: Vec<u64> = cbf_peers
            .iter()
            .map(|address| {
                let response = manager.receive_from(address, "pong").unwrap().unwrap();

                let value = match response {
                    NetworkMessage::Pong(v) => Some(v),
                    _ => None,
                };

                value.unwrap()
            })
            .collect();

        let non_cbf_received: Vec<u64> = non_cbf_peers
            .iter()
            .map(|address| {
                let response = manager.receive_from(address, "pong").unwrap().unwrap();

                let value = match response {
                    NetworkMessage::Pong(v) => Some(v),
                    _ => None,
                };

                value.unwrap()
            })
            .collect();

        assert_eq!(cbf_pings, cbf_received);

        assert_eq!(non_cbf_pings, non_cbf_received);
    }
}
