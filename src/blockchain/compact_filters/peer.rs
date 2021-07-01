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

use std::collections::HashMap;
use std::fmt;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use std::sync::PoisonError;
use std::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult};

use socks::{Socks5Stream, ToTargetAddr};

use rand::{thread_rng, Rng};

use bitcoin::consensus::Encodable;
use bitcoin::hash_types::BlockHash;
use bitcoin::network::constants::ServiceFlags;
use bitcoin::network::message::{NetworkMessage, RawNetworkMessage};
use bitcoin::network::message_blockdata::*;
use bitcoin::network::message_filter::*;
use bitcoin::network::message_network::VersionMessage;
use bitcoin::network::stream_reader::StreamReader;
use bitcoin::network::Address;
use bitcoin::{Block, Network, Transaction, Txid, Wtxid};

type ResponsesMap = HashMap<&'static str, Arc<(Mutex<Vec<NetworkMessage>>, Condvar)>>;

pub(crate) const TIMEOUT_SECS: u64 = 30;

/// Container for unconfirmed, but valid Bitcoin transactions
///
/// It is normally shared between [`Peer`]s with the use of [`Arc`], so that transactions are not
/// duplicated in memory.
#[derive(Debug, Default)]
pub struct Mempool(RwLock<InnerMempool>);

#[derive(Debug, Default)]
struct InnerMempool {
    txs: HashMap<Txid, Transaction>,
    wtxids: HashMap<Wtxid, Txid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TxIdentifier {
    Wtxid(Wtxid),
    Txid(Txid),
}

impl Mempool {
    /// Create a new empty mempool
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a transaction to the mempool
    ///
    /// Note that this doesn't propagate the transaction to other
    /// peers. To do that, [`broadcast`](crate::blockchain::Blockchain::broadcast) should be used.
    pub fn add_tx(&self, tx: Transaction) -> Result<(), PeerError> {
        let mut guard = self.0.write()?;

        guard.wtxids.insert(tx.wtxid(), tx.txid());
        guard.txs.insert(tx.txid(), tx);
        Ok(())
    }

    /// Look-up a transaction in the mempool given an [`Inventory`] request
    pub fn get_tx(&self, inventory: &Inventory) -> Result<Option<Transaction>, PeerError> {
        let identifer = match inventory {
            Inventory::Error | Inventory::Block(_) | Inventory::WitnessBlock(_) => return Ok(None),
            Inventory::Transaction(txid) => TxIdentifier::Txid(*txid),
            Inventory::WitnessTransaction(txid) => TxIdentifier::Txid(*txid),
            Inventory::WTx(wtxid) => TxIdentifier::Wtxid(*wtxid),
            Inventory::Unknown { inv_type, hash } => {
                log::warn!(
                    "Unknown inventory request type `{}`, hash `{:?}`",
                    inv_type,
                    hash
                );
                return Ok(None);
            }
        };

        let txid = match identifer {
            TxIdentifier::Txid(txid) => Some(txid),
            TxIdentifier::Wtxid(wtxid) => self.0.read()?.wtxids.get(&wtxid).cloned(),
        };

        let result = match txid {
            Some(txid) => {
                let read_lock = self.0.read()?;
                read_lock.txs.get(&txid).cloned()
            }
            None => None,
        };

        Ok(result)
    }

    /// Return whether or not the mempool contains a transaction with a given txid
    pub fn has_tx(&self, txid: &Txid) -> Result<bool, PeerError> {
        Ok(self.0.read()?.txs.contains_key(txid))
    }

    /// Return the list of transactions contained in the mempool
    pub fn iter_txs(&self) -> Result<Vec<Transaction>, PeerError> {
        Ok(self.0.read()?.txs.values().cloned().collect())
    }
}

/// A Bitcoin peer
#[derive(Debug)]
pub struct Peer {
    writer: Arc<Mutex<TcpStream>>,
    responses: Arc<RwLock<ResponsesMap>>,

    reader_thread: thread::JoinHandle<()>,
    connected: Arc<RwLock<bool>>,

    mempool: Arc<Mempool>,

    version: VersionMessage,
    network: Network,
}

impl Peer {
    /// Connect to a peer over a plaintext TCP connection
    ///
    /// This function internally spawns a new thread that will monitor incoming messages from the
    /// peer, and optionally reply to some of them transparently, like [pings](bitcoin::network::message::NetworkMessage::Ping)
    pub fn connect<A: ToSocketAddrs>(
        address: A,
        mempool: Arc<Mempool>,
        network: Network,
    ) -> Result<Self, PeerError> {
        let stream = TcpStream::connect(address)?;

        Peer::from_stream(stream, mempool, network)
    }

    /// Connect to a peer over a plaintext TCP connection with a timeout
    ///
    /// This function behaves exactly the same as `connect` except for two differences
    /// 1) It assumes your ToSocketAddrs will resolve to a single address
    /// 2) It lets you specify a connection timeout
    pub fn connect_with_timeout<A: ToSocketAddrs>(
        address: A,
        timeout: Duration,
        mempool: Arc<Mempool>,
        network: Network,
    ) -> Result<Self, PeerError> {
        let socket_addr = address
            .to_socket_addrs()?
            .next()
            .ok_or(PeerError::AddresseResolution)?;
        let stream = TcpStream::connect_timeout(&socket_addr, timeout)?;
        Peer::from_stream(stream, mempool, network)
    }

    /// Connect to a peer through a SOCKS5 proxy, optionally by using some credentials, specified
    /// as a tuple of `(username, password)`
    ///
    /// This function internally spawns a new thread that will monitor incoming messages from the
    /// peer, and optionally reply to some of them transparently, like [pings](NetworkMessage::Ping)
    pub fn connect_proxy<T: ToTargetAddr, P: ToSocketAddrs>(
        target: T,
        proxy: P,
        credentials: Option<(&str, &str)>,
        mempool: Arc<Mempool>,
        network: Network,
    ) -> Result<Self, PeerError> {
        let socks_stream = if let Some((username, password)) = credentials {
            Socks5Stream::connect_with_password(proxy, target, username, password)?
        } else {
            Socks5Stream::connect(proxy, target)?
        };

        Peer::from_stream(socks_stream.into_inner(), mempool, network)
    }

    /// Create a [`Peer`] from an already connected TcpStream
    fn from_stream(
        stream: TcpStream,
        mempool: Arc<Mempool>,
        network: Network,
    ) -> Result<Self, PeerError> {
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let responses: Arc<RwLock<ResponsesMap>> = Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(RwLock::new(true));

        let mut locked_writer = writer.lock()?;

        let reader_thread_responses = Arc::clone(&responses);
        let reader_thread_writer = Arc::clone(&writer);
        let reader_thread_mempool = Arc::clone(&mempool);
        let reader_thread_connected = Arc::clone(&connected);
        let reader_thread = thread::spawn(move || {
            Self::reader_thread(
                network,
                stream,
                reader_thread_responses,
                reader_thread_writer,
                reader_thread_mempool,
                reader_thread_connected,
            )
            .unwrap()
        });

        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        let nonce = thread_rng().gen();
        let receiver = Address::new(&locked_writer.peer_addr()?, ServiceFlags::NONE);
        let sender = Address {
            services: ServiceFlags::NONE,
            address: [0u16; 8],
            port: 0,
        };

        Self::_send(
            &mut locked_writer,
            network.magic(),
            NetworkMessage::Version(VersionMessage::new(
                ServiceFlags::WITNESS,
                timestamp,
                receiver,
                sender,
                nonce,
                "MagicalBitcoinWallet".into(),
                0,
            )),
        )?;

        let version = match Self::_recv(&responses, "version", Some(Duration::from_secs(1)))? {
            Some(NetworkMessage::Version(version)) => version,
            _ => {
                return Err(PeerError::InvalidResponse(locked_writer.peer_addr()?));
            }
        };

        if let Some(NetworkMessage::Verack) =
            Self::_recv(&responses, "verack", Some(Duration::from_secs(1)))?
        {
            Self::_send(&mut locked_writer, network.magic(), NetworkMessage::Verack)?;
        } else {
            return Err(PeerError::InvalidResponse(locked_writer.peer_addr()?));
        }

        std::mem::drop(locked_writer);

        Ok(Peer {
            writer,
            responses,
            reader_thread,
            connected,
            mempool,
            version,
            network,
        })
    }

    /// Close the peer connection
    // Consume Self
    pub fn close(self) -> Result<(), PeerError> {
        let locked_writer = self.writer.lock()?;
        Ok((*locked_writer).shutdown(std::net::Shutdown::Both)?)
    }

    /// Get the socket address of the remote peer
    pub fn get_address(&self) -> Result<SocketAddr, PeerError> {
        let locked_writer = self.writer.lock()?;
        Ok(locked_writer.peer_addr()?)
    }

    /// Send a Bitcoin network message
    fn _send(writer: &mut TcpStream, magic: u32, payload: NetworkMessage) -> Result<(), PeerError> {
        log::trace!("==> {:?}", payload);

        let raw_message = RawNetworkMessage { magic, payload };

        raw_message.consensus_encode(writer)?;

        Ok(())
    }

    /// Wait for a specific incoming Bitcoin message, optionally with a timeout
    fn _recv(
        responses: &Arc<RwLock<ResponsesMap>>,
        wait_for: &'static str,
        timeout: Option<Duration>,
    ) -> Result<Option<NetworkMessage>, PeerError> {
        let message_resp = {
            let mut lock = responses.write()?;
            let message_resp = lock.entry(wait_for).or_default();
            Arc::clone(&message_resp)
        };

        let (lock, cvar) = &*message_resp;

        let mut messages = lock.lock()?;
        while messages.is_empty() {
            match timeout {
                None => messages = cvar.wait(messages)?,
                Some(t) => {
                    let result = cvar.wait_timeout(messages, t)?;
                    if result.1.timed_out() {
                        return Ok(None);
                    }
                    messages = result.0;
                }
            }
        }

        Ok(messages.pop())
    }

    /// Return the [`VersionMessage`] sent by the peer
    pub fn get_version(&self) -> &VersionMessage {
        &self.version
    }

    /// Return the Bitcoin [`Network`] in use
    pub fn get_network(&self) -> Network {
        self.network
    }

    /// Return the mempool used by this peer
    pub fn get_mempool(&self) -> Arc<Mempool> {
        Arc::clone(&self.mempool)
    }

    /// Return whether or not the peer is still connected
    pub fn is_connected(&self) -> Result<bool, PeerError> {
        Ok(*self.connected.read()?)
    }

    /// Internal function called once the `reader_thread` is spawned
    fn reader_thread(
        network: Network,
        connection: TcpStream,
        reader_thread_responses: Arc<RwLock<ResponsesMap>>,
        reader_thread_writer: Arc<Mutex<TcpStream>>,
        reader_thread_mempool: Arc<Mempool>,
        reader_thread_connected: Arc<RwLock<bool>>,
    ) -> Result<(), PeerError> {
        macro_rules! check_disconnect {
            ($call:expr) => {
                match $call {
                    Ok(good) => good,
                    Err(e) => {
                        log::debug!("Error {:?}", e);
                        *reader_thread_connected.write()? = false;

                        break;
                    }
                }
            };
        }

        let mut reader = StreamReader::new(connection, None);
        while *reader_thread_connected.read()? {
            let raw_message: RawNetworkMessage = check_disconnect!(reader.read_next());

            let in_message = if raw_message.magic != network.magic() {
                continue;
            } else {
                raw_message.payload
            };

            log::trace!("<== {:?}", in_message);

            match in_message {
                NetworkMessage::Ping(nonce) => {
                    check_disconnect!(Self::_send(
                        &mut *reader_thread_writer.lock()?,
                        network.magic(),
                        NetworkMessage::Pong(nonce),
                    ));

                    continue;
                }
                NetworkMessage::Alert(_) => continue,
                NetworkMessage::GetData(ref inv) => {
                    let (found, not_found): (Vec<_>, Vec<_>) = inv
                        .iter()
                        .map(|item| (*item, reader_thread_mempool.get_tx(item).unwrap()))
                        .partition(|(_, d)| d.is_some());
                    for (_, found_tx) in found {
                        check_disconnect!(Self::_send(
                            &mut *reader_thread_writer.lock()?,
                            network.magic(),
                            NetworkMessage::Tx(found_tx.ok_or_else(|| PeerError::Generic(
                                "Got None while expecting Transaction".to_string()
                            ))?),
                        ));
                    }

                    if !not_found.is_empty() {
                        check_disconnect!(Self::_send(
                            &mut *reader_thread_writer.lock()?,
                            network.magic(),
                            NetworkMessage::NotFound(
                                not_found.into_iter().map(|(i, _)| i).collect(),
                            ),
                        ));
                    }
                }
                _ => {}
            }

            let message_resp = {
                let mut lock = reader_thread_responses.write()?;
                let message_resp = lock.entry(in_message.cmd()).or_default();
                Arc::clone(&message_resp)
            };

            let (lock, cvar) = &*message_resp;
            let mut messages = lock.lock()?;
            messages.push(in_message);
            cvar.notify_all();
        }

        Ok(())
    }

    /// Send a raw Bitcoin message to the peer
    pub fn send(&self, payload: NetworkMessage) -> Result<(), PeerError> {
        let mut writer = self.writer.lock()?;
        Self::_send(&mut writer, self.network.magic(), payload)
    }

    /// Waits for a specific incoming Bitcoin message, optionally with a timeout
    pub fn recv(
        &self,
        wait_for: &'static str,
        timeout: Option<Duration>,
    ) -> Result<Option<NetworkMessage>, PeerError> {
        Self::_recv(&self.responses, wait_for, timeout)
    }
}

pub trait CompactFiltersPeer {
    fn get_cf_checkpt(&self, filter_type: u8, stop_hash: BlockHash)
        -> Result<CFCheckpt, PeerError>;
    fn get_cf_headers(
        &self,
        filter_type: u8,
        start_height: u32,
        stop_hash: BlockHash,
    ) -> Result<CFHeaders, PeerError>;
    fn get_cf_filters(
        &self,
        filter_type: u8,
        start_height: u32,
        stop_hash: BlockHash,
    ) -> Result<(), PeerError>;
    fn pop_cf_filter_resp(&self) -> Result<CFilter, PeerError>;
}

impl CompactFiltersPeer for Peer {
    fn get_cf_checkpt(
        &self,
        filter_type: u8,
        stop_hash: BlockHash,
    ) -> Result<CFCheckpt, PeerError> {
        self.send(NetworkMessage::GetCFCheckpt(GetCFCheckpt {
            filter_type,
            stop_hash,
        }))?;

        let response = self.recv("cfcheckpt", Some(Duration::from_secs(TIMEOUT_SECS)))?;
        let response = match response {
            Some(NetworkMessage::CFCheckpt(response)) => response,
            _ => return Err(PeerError::InvalidResponse(self.get_address()?)),
        };

        if response.filter_type != filter_type {
            return Err(PeerError::InvalidResponse(self.get_address()?));
        }

        Ok(response)
    }

    fn get_cf_headers(
        &self,
        filter_type: u8,
        start_height: u32,
        stop_hash: BlockHash,
    ) -> Result<CFHeaders, PeerError> {
        self.send(NetworkMessage::GetCFHeaders(GetCFHeaders {
            filter_type,
            start_height,
            stop_hash,
        }))?;

        let response = self.recv("cfheaders", Some(Duration::from_secs(TIMEOUT_SECS)))?;
        let response = match response {
            Some(NetworkMessage::CFHeaders(response)) => response,
            _ => return Err(PeerError::InvalidResponse(self.get_address()?)),
        };

        if response.filter_type != filter_type {
            return Err(PeerError::InvalidResponse(self.get_address()?));
        }

        Ok(response)
    }

    fn pop_cf_filter_resp(&self) -> Result<CFilter, PeerError> {
        let response = self.recv("cfilter", Some(Duration::from_secs(TIMEOUT_SECS)))?;
        let response = match response {
            Some(NetworkMessage::CFilter(response)) => response,
            _ => return Err(PeerError::InvalidResponse(self.get_address()?)),
        };

        Ok(response)
    }

    fn get_cf_filters(
        &self,
        filter_type: u8,
        start_height: u32,
        stop_hash: BlockHash,
    ) -> Result<(), PeerError> {
        self.send(NetworkMessage::GetCFilters(GetCFilters {
            filter_type,
            start_height,
            stop_hash,
        }))?;

        Ok(())
    }
}

pub trait InvPeer {
    fn get_block(&self, block_hash: BlockHash) -> Result<Option<Block>, PeerError>;
    fn ask_for_mempool(&self) -> Result<(), PeerError>;
    fn broadcast_tx(&self, tx: Transaction) -> Result<(), PeerError>;
}

impl InvPeer for Peer {
    fn get_block(&self, block_hash: BlockHash) -> Result<Option<Block>, PeerError> {
        self.send(NetworkMessage::GetData(vec![Inventory::WitnessBlock(
            block_hash,
        )]))?;

        match self.recv("block", Some(Duration::from_secs(TIMEOUT_SECS)))? {
            None => Ok(None),
            Some(NetworkMessage::Block(response)) => Ok(Some(response)),
            _ => Err(PeerError::InvalidResponse(self.get_address()?)),
        }
    }

    fn ask_for_mempool(&self) -> Result<(), PeerError> {
        if !self.version.services.has(ServiceFlags::BLOOM) {
            return Err(PeerError::PeerBloomDisabled(self.get_address()?));
        }

        self.send(NetworkMessage::MemPool)?;
        let inv = match self.recv("inv", Some(Duration::from_secs(5)))? {
            None => return Ok(()), // empty mempool
            Some(NetworkMessage::Inv(inv)) => inv,
            _ => return Err(PeerError::InvalidResponse(self.get_address()?)),
        };

        let getdata = inv
            .iter()
            .cloned()
            .filter(
                |item| matches!(item, Inventory::Transaction(txid) if !self.mempool.has_tx(txid).unwrap()),
            )
            .collect::<Vec<_>>();
        let num_txs = getdata.len();
        self.send(NetworkMessage::GetData(getdata))?;

        for _ in 0..num_txs {
            let tx = self.recv("tx", Some(Duration::from_secs(TIMEOUT_SECS)))?;
            let tx = match tx {
                Some(NetworkMessage::Tx(tx)) => tx,
                _ => return Err(PeerError::InvalidResponse(self.get_address()?)),
            };

            self.mempool.add_tx(tx)?;
        }

        Ok(())
    }

    fn broadcast_tx(&self, tx: Transaction) -> Result<(), PeerError> {
        self.mempool.add_tx(tx.clone())?;
        self.send(NetworkMessage::Tx(tx))?;

        Ok(())
    }
}

/// Peer Errors
#[derive(Debug)]
pub enum PeerError {
    /// Internal I/O error
    Io(std::io::Error),

    /// Internal system time error
    Time(std::time::SystemTimeError),

    /// A peer sent an invalid or unexpected response
    InvalidResponse(SocketAddr),

    /// Peer had bloom filter disabled
    PeerBloomDisabled(SocketAddr),

    /// Internal Mutex poisoning error
    MutexPoisoned,

    /// Internal Mutex wait timed out
    MutexTimedout,

    /// Internal RW read lock poisoned
    RwReadLockPoisined,

    /// Internal RW write lock poisoned
    RwWriteLockPoisoned,

    /// Mempool Mutex poisoned
    MempoolPoisoned,

    /// Network address resolution Error
    AddresseResolution,

    /// Generic Errors
    Generic(String),
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for PeerError {}

impl_error!(std::io::Error, Io, PeerError);
impl_error!(std::time::SystemTimeError, Time, PeerError);

impl<T> From<PoisonError<MutexGuard<'_, T>>> for PeerError {
    fn from(_: PoisonError<MutexGuard<'_, T>>) -> Self {
        PeerError::MutexPoisoned
    }
}

impl<T> From<PoisonError<RwLockWriteGuard<'_, T>>> for PeerError {
    fn from(_: PoisonError<RwLockWriteGuard<'_, T>>) -> Self {
        PeerError::RwWriteLockPoisoned
    }
}

impl<T> From<PoisonError<RwLockReadGuard<'_, T>>> for PeerError {
    fn from(_: PoisonError<RwLockReadGuard<'_, T>>) -> Self {
        PeerError::RwReadLockPoisined
    }
}

impl<T> From<PoisonError<(MutexGuard<'_, T>, WaitTimeoutResult)>> for PeerError {
    fn from(err: PoisonError<(MutexGuard<'_, T>, WaitTimeoutResult)>) -> Self {
        let (_, wait_result) = err.into_inner();
        if wait_result.timed_out() {
            PeerError::MutexTimedout
        } else {
            PeerError::MutexPoisoned
        }
    }
}
