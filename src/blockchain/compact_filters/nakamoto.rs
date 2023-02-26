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

//! This is Compact Filter (BIP157) Blockchain implementation using the nakamoto crate.
//! https://github.com/cloudhead/nakamoto
//!
//! This module implements BDK's [crate::blockchain::Blockchain] trait over a nakamoto
//! client. Which is it's own state machine that maintains the compact filter database
//! and interface with the Bitcoin p2p network using the BIP157/BIP158 protocol.
#![allow(dead_code)]

use crate::blockchain::{
    Blockchain, Capability, ConfigurableBlockchain, GetBlockHash, GetHeight, GetTx, WalletSync,
};
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::{BlockTime, FeeRate, KeychainKind, LocalUtxo, TransactionDetails};
use bitcoin::{Block, OutPoint, Script, Transaction, Txid};
use log::{debug, info};
use nakamoto::p2p::fsm::fees::FeeEstimate;
use nakamoto::{
    client::{
        chan::Receiver,
        handle::Handle,
        Client,
        Config,
        Event,
        Handle as ClientHandle,
        //event::TxStatus
    },
    net::poll::Waker,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::ops::DerefMut;
use std::path::PathBuf;
use std::time::Duration;
use std::{net::TcpStream, thread};
use thiserror::Error;

type Reactor = nakamoto::net::poll::Reactor<TcpStream>;

/// An Error that occurred during the compact block filter operation.
#[derive(Debug, Error)]
pub enum CbfError {
    /// Global BDK error
    #[error(transparent)]
    Global(#[from] Box<crate::error::Error>),

    /// Nakamoto client error
    #[error(transparent)]
    Nakamoto(#[from] nakamoto::client::Error),
}

impl From<crate::error::Error> for CbfError {
    fn from(err: crate::error::Error) -> Self {
        CbfError::Global(Box::new(err))
    }
}

/// Process a transaction by looking for inputs that spend from a UTXO in the database or
/// outputs that send funds to a know script_pubkey.
fn add_tx(
    database: &mut impl BatchDatabase,
    tx: &Transaction,
    height: Option<u32>,
    timestamp: Option<u64>,
    internal_max_deriv: &mut Option<u32>,
    external_max_deriv: &mut Option<u32>,
) -> Result<(), CbfError> {
    let mut updates = database.begin_batch();

    let mut incoming: u64 = 0;
    let mut outgoing: u64 = 0;
    let mut inputs_sum: u64 = 0;
    let mut outputs_sum: u64 = 0;

    // Sanity check for conflicts
    let db_txs = database.iter_raw_txs()?;
    let conflicts = db_txs
        .iter()
        .filter_map(|existing_tx| {
            let existing_inputs = existing_tx
                .input
                .iter()
                .map(|txin| txin.previous_output)
                .collect::<HashSet<_>>();
            let current_inputs = tx
                .input
                .iter()
                .map(|txin| txin.previous_output)
                .collect::<HashSet<_>>();
            if !current_inputs
                .intersection(&existing_inputs)
                .collect::<Vec<_>>()
                .is_empty()
                && existing_tx.txid() != tx.txid()
            {
                Some(existing_tx)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for conflict in conflicts {
        debug!(
            "{} Conflicts with existing tx {}",
            tx.txid(),
            conflict.txid()
        );
        // TODO: Make more intelligent checking following RBF rules.
        if conflict.is_explicitly_rbf() {
            debug!("{} Replaces {} via RBF", tx.txid(), conflict.txid());
            // Assuming the latest RBF transaction is the right transaction to keep.
            // Delete the existing one.
            delete_tx(database, &conflict.txid())?;
        } else {
            debug!(
                "{} is non-rbf and conflicts with {}. Rejected",
                tx.txid(),
                conflict.txid()
            );
            return Ok(());
        }
    }

    // Process transaction inputs
    for input in tx.input.iter() {
        if let Some(previous_output) = database.get_previous_output(&input.previous_output)? {
            inputs_sum += previous_output.value;

            // this output is ours, we have a path to derive it
            if let Some((keychain, _)) =
                database.get_path_from_script_pubkey(&previous_output.script_pubkey)?
            {
                outgoing += previous_output.value;

                debug!("{} is mine, setting utxo as spent", input.previous_output);
                updates.set_utxo(&LocalUtxo {
                    outpoint: input.previous_output,
                    txout: previous_output.clone(),
                    keychain,
                    is_spent: true,
                })?;
            }
        }
    }

    // Process transaction outputs
    for (i, output) in tx.output.iter().enumerate() {
        outputs_sum += output.value;

        // When the output is ours
        if let Some((keychain, child)) =
            database.get_path_from_script_pubkey(&output.script_pubkey)?
        {
            debug!("{} output #{} is mine, adding utxo", tx.txid(), i);
            updates.set_utxo(&LocalUtxo {
                outpoint: OutPoint::new(tx.txid(), i as u32),
                txout: output.clone(),
                keychain,
                is_spent: false,
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
            confirmation_time: BlockTime::new(height, timestamp),
            fee: Some(inputs_sum.saturating_sub(outputs_sum)),
        };

        debug!("Saving tx {}", tx.txid);
        updates.set_tx(&tx)?;
    }

    database.commit_batch(updates)?;

    Ok(())
}

/// Mark as transaction as unconfirmed in the database.
/// This is called in block reorg cases, where previously confirmed transactions becomes unconfirmed.
fn unconfirm_tx(
    database: &mut impl BatchDatabase,
    txid: &Txid,
) -> Result<TransactionDetails, CbfError> {
    let mut tx_details = database
        .get_tx(&txid, false)?
        .expect("We must have the transaction at this stage");
    tx_details.confirmation_time = None;
    database.set_tx(&tx_details)?;
    debug!("Existing transaction marked unconfirmed : {}", txid);
    Ok(tx_details)
}

/// Delete a Transaction from the database, updating related UTXO records.
/// This is called in a a RBF case.
fn delete_tx(database: &mut impl BatchDatabase, txid: &Txid) -> Result<(), CbfError> {
    let mut updates = database.begin_batch();

    if let Some(db_tx) = database.get_raw_tx(txid)? {
        let txid = db_tx.txid();
        // Mark input utxos as unspent
        for input in db_tx.input {
            let prev_output = input.previous_output;
            if let Some(mut existing_utxo) = database.get_utxo(&prev_output)? {
                debug!("Setting UTXO : {} as `unspent`", existing_utxo.outpoint);
                existing_utxo.is_spent = false;
                updates.set_utxo(&existing_utxo)?;
            }
        }

        // Delete output utxos
        for (i, _) in db_tx.output.iter().enumerate() {
            let outpoint = OutPoint::new(txid, i as u32);
            if let Some(deleted_utxo) = updates.del_utxo(&outpoint)? {
                debug!("Deleting Existing UTXO {}", deleted_utxo.outpoint);
            }
        }

        // Delete the transaction data
        let _ = database
            .del_tx(&txid, true)?
            .expect("We must have the transaction at this stage")
            .txid;
        debug!("Deleting existing transaction : txid: {}", txid);
    }

    database.commit_batch(updates)?;

    Ok(())
}

/// Structure containing the nakamoto client handle and event receiver.
pub struct CbfBlockchain {
    /// A Receiver of all client related events
    receiver: Receiver<Event>,
    /// The handler for the client
    client_handle: ClientHandle<Waker>,
    /// Internal query timeout. Set by default at 60 secs.
    timeout: Duration,
    /// A store of blockheight and fee estimation
    fee_data: Cell<HashMap<u32, FeeEstimate>>,
    /// A local cache of unconfirmed broadcasted transactions.
    /// Used to sync wallet db with known unconfirmed txs. Cleared at each `wallet.sync()` call.
    broadcasted_txs: Cell<Vec<Transaction>>,
    /// Last Sync height
    last_sync_height: Cell<u32>,
    // A Hack to handle reorg sync cases.
    #[cfg(test)]
    break_sync_at: Cell<Option<u32>>,
}

/// Nakamoto CBF Client Configuration
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct CBFBlockchainConfig {
    /// Network for the CBF client
    pub network: bitcoin::Network,
    /// Optional custom datadir for CBF Client
    pub datadir: Option<PathBuf>,
    /// Initial Peer List
    pub peers: Vec<SocketAddr>,
}

impl CbfBlockchain {
    /// Create a new CBF node which runs at the background.
    pub fn new(
        network: bitcoin::Network,
        datadir: Option<PathBuf>,
        peers: Vec<SocketAddr>,
    ) -> Result<Self, CbfError> {
        let root = if let Some(dir) = datadir {
            dir
        } else {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
        };
        let cbf_client = Client::<Reactor>::new()?;
        let client_cfg = Config {
            network: network.into(),
            listen: vec![], // Don't listen for incoming connections.
            root,
            ..Config::default()
        };

        let client_handle = cbf_client.handle();
        thread::spawn(|| {
            cbf_client.run(client_cfg).unwrap();
        });
        let receiver = client_handle.events();

        for peer in peers {
            let peer_link = client_handle
                .connect(peer)
                .map_err(nakamoto::client::Error::from)?;
            debug!("New peer connected : {:?}", peer_link);
        }

        Ok(Self {
            receiver,
            client_handle,
            timeout: Duration::from_secs(60), // This is nakamoto default client timeout
            fee_data: Cell::new(HashMap::new()),
            broadcasted_txs: Cell::new(Vec::new()),
            last_sync_height: Cell::new(0u32),
            #[cfg(test)]
            break_sync_at: Cell::new(None),
        })
    }

    /// Scan the filters from a height, for a given list of scripts
    pub fn scan(&self, from: u32, scripts: Vec<Script>) {
        let _ = self
            .client_handle
            .rescan((from as u64).., scripts.into_iter());
    }

    /// Add fee estimation data, in the fee estimation store
    fn add_fee_data(&self, height: u32, fee_estimate: FeeEstimate) {
        let mut data = self.fee_data.take();
        data.insert(height, fee_estimate);
        self.fee_data.set(data)
    }

    // /// Connect the client with a specific peer.
    // pub fn add_peers(&self, peers: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, CbfError> {
    //     let mut connected_peers = vec![];
    //     for peer in peers {
    //         let _ = self
    //             .client_handle
    //             .connect(peer)
    //             .map_err(nakamoto::client::Error::from)?;
    //         connected_peers.push(peer);
    //     }
    //     Ok(connected_peers)
    // }

    /// Disconnect the client from a specific peer
    pub fn diconnect(&self, peer: SocketAddr) {
        self.client_handle.disconnect(peer).unwrap();
    }

    /// Broadcast a transaction to the p2p network
    fn braodcast(&self, tx: Transaction) -> Result<(), CbfError> {
        debug!("Broadcasting txid : {}", tx.txid(),);
        let socket = self
            .client_handle
            .submit_transaction(tx.clone())
            .map_err(nakamoto::client::Error::from)?;
        debug!("Broacasted to : {:?}", socket);
        // Add the tx to the broadcasted list
        let mut broadcasted = self.broadcasted_txs.take();
        broadcasted.push(tx);
        self.broadcasted_txs.set(broadcasted);

        Ok(())
    }

    /// Read the next event received from event listener
    pub fn get_next_event(&self) -> Result<Event, CbfError> {
        Ok(self
            .receiver
            .recv()
            .map_err(|e| nakamoto::client::Error::from(nakamoto::client::handle::Error::from(e)))?)
    }

    /// Shutdown the client
    pub fn shutdown(self) {
        self.client_handle.shutdown().unwrap();
    }

    /// Get a Block from the p2p network given a height
    pub fn get_block(&self, height: u32) -> Result<Option<Block>, CbfError> {
        if let Some(header) = self
            .client_handle
            .get_block_by_height(height as u64)
            .map_err(nakamoto::client::Error::from)?
        {
            self.client_handle
                .get_block(&header.block_hash())
                .map_err(nakamoto::client::Error::from)?;

            let receiver = self.client_handle.blocks();

            let block;
            loop {
                let (new_block, blk_height) = receiver.recv().map_err(|e| {
                    nakamoto::client::Error::from(nakamoto::client::handle::Error::from(e))
                })?;
                if blk_height as u32 == height {
                    block = new_block;
                    break;
                }
            }

            Ok(Some(block))
        } else {
            Ok(None)
        }
    }

    #[cfg(test)]
    pub fn set_break_sync_height(&self, ht: u32) {
        let height = Some(ht);
        self.break_sync_at.set(height);
    }
}

impl Blockchain for CbfBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![Capability::FullHistory].into_iter().collect()
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), crate::Error> {
        self.braodcast(tx.clone())?;
        Ok(())
    }

    // TODO: Improve Fee estimation
    fn estimate_fee(&self, _target: usize) -> Result<FeeRate, crate::Error> {
        // Return the last known fee estimation
        let fee_data = self.fee_data.take();
        let last_block_fee_rate = fee_data
            .iter()
            .last()
            .map(|(_, estimate)| {
                let median_rate = FeeRate::from_sat_per_vb(estimate.median as f32);
                median_rate
            })
            .expect("Some value expected");
        self.fee_data.set(fee_data);
        Ok(last_block_fee_rate)
    }
}

impl GetBlockHash for CbfBlockchain {
    fn get_block_hash(&self, height: u64) -> Result<bitcoin::BlockHash, crate::Error> {
        let header = self
            .client_handle
            .get_block_by_height(height)
            .map_err(nakamoto::client::Error::from)
            .map_err(CbfError::from)?
            .expect("Block at given height doesn't exist");
        Ok(header.block_hash())
    }
}

impl GetTx for CbfBlockchain {
    fn get_tx(&self, _txid: &Txid) -> Result<Option<Transaction>, crate::Error> {
        Ok(None)
    }
}

impl GetHeight for CbfBlockchain {
    fn get_height(&self) -> Result<u32, crate::Error> {
        Ok(self
            .client_handle
            .get_tip()
            .map_err(nakamoto::client::Error::from)
            .map_err(CbfError::from)?
            .0 as u32)
    }
}

impl WalletSync for CbfBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn crate::blockchain::Progress>,
    ) -> Result<(), crate::Error> {
        let mut database = database.borrow_mut();
        let database = database.deref_mut();
        let db_scripts = database.iter_script_pubkeys(None)?;
        self.client_handle
            .watch(db_scripts.iter().cloned())
            .map_err(nakamoto::client::Error::from)
            .map_err(CbfError::from)?;

        // Rescan from previous block of last_sync_height, to trigger the event loop.
        self.scan(self.last_sync_height.get().saturating_sub(1), db_scripts);

        let mut internal_max_deriv = None;
        let mut external_max_deriv = None;

        // Add all broadcasted transactions to db
        for tx in self.broadcasted_txs.take().iter() {
            debug!("Processing braodcasted transaction : {}", tx.txid());
            add_tx(
                database,
                tx,
                None,
                None,
                &mut internal_max_deriv,
                &mut external_max_deriv,
            )?;
        }

        loop {
            // TODO: Investigate why this is causing error
            // if self.last_sync_height.get() == self.get_height()? {
            //     break;
            // }
            match self.get_next_event()? {
                Event::BlockConnected { hash, height, .. } => {
                    debug!("New Block Found : {} at {}", hash, height);
                }
                Event::BlockDisconnected { height, .. } => {
                    let db_txs = database
                        .iter_txs(false)?
                        .iter()
                        .filter_map(|tx_details| {
                            if let Some(block_time) = &tx_details.confirmation_time {
                                if block_time.height == height as u32 {
                                    Some(tx_details.txid)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect::<HashSet<_>>();

                    for txid in db_txs {
                        unconfirm_tx(database, &txid)?;
                    }
                }
                Event::BlockMatched {
                    hash: _,
                    header,
                    height,
                    transactions,
                } => {
                    let timestamp = header.time;
                    for tx in transactions {
                        // Check that we haven't processed this transaction before
                        if database
                            .iter_txs(false)?
                            .iter()
                            .find(|detail| {
                                detail.txid == tx.txid()
                                    && detail.confirmation_time.as_ref().map(|bt| bt.height)
                                        == Some(height as u32)
                            })
                            .is_none()
                        {
                            debug!(
                                "Processing Confirmed Transaction : {} at Block : {}",
                                tx.txid(),
                                height
                            );
                            add_tx(
                                database,
                                &tx,
                                Some(height as u32),
                                Some(timestamp as u64),
                                &mut internal_max_deriv,
                                &mut external_max_deriv,
                            )?;
                        }
                    }
                }
                Event::FeeEstimated { height, fees, .. } => {
                    self.add_fee_data(height as u32, fees);
                }
                // Event::TxStatusChanged { txid, status } => {
                //     match status {
                //         TxStatus::Unconfirmed => {
                //             debug!("txid:{}, status : Broadcasted", txid);
                //         }
                //         TxStatus::Acknowledged { peer } => {
                //             debug!("txid: {}, status : ACK by {}", txid, peer);
                //         }
                //         TxStatus::Confirmed { height, .. } => {
                //             debug!("txid: {}, status : Confirmed at {}", txid, height);
                //         }
                //         TxStatus::Reverted => {
                //             debug!("Transaction reverted due to reorg: txid : {}", txid);
                //             let _ = unconfirm_tx(database, &txid)?;
                //         }
                //         TxStatus::Stale { replaced_by, block } => {
                //             debug!(
                //                 "txid: {}, status : Replaced by : txid {} at blockhash {}",
                //                 txid, replaced_by, block
                //             );
                //             let tx = database.get_tx(&replaced_by, true)?.expect(
                //                 "Wallet transaction got replaced by unknown foreign transaction",
                //             ); // TODO: Handle the case when we don't have the overriding transaction
                //             delete_tx(database, &tx.txid)?;
                //         }
                //     }
                // }
                Event::Synced { height, tip } => {
                    debug!("Sync Status : {}:{}", height, tip);
                    progress_update.update(
                        (height as f32 / tip as f32) * 100.0,
                        Some("Sync Progress".into()),
                    )?;
                    // We break the sync loop once we reach current height.
                    if height == tip {
                        debug!("Sync complete at : {}", height);
                        self.last_sync_height.set(height as u32);
                        break;
                    }
                    // Check for force stop signal
                    #[cfg(test)]
                    {
                        if let Some(break_height) = self.break_sync_at.get() {
                            if break_height == height as u32 {
                                debug!("Force breaking sync at : {}", height);
                                self.last_sync_height.set(height as u32);
                                break;
                            }
                        }
                    }
                }
                _ => { /* Rest of the Events are unused */ }
            }
        }

        // Set External Last index after sync
        let current_ext = database
            .get_last_index(KeychainKind::External)?
            .unwrap_or(0);
        let first_ext_new = external_max_deriv.unwrap_or(0);
        if first_ext_new > current_ext {
            debug!("Setting external index to {}", first_ext_new);
            database.set_last_index(KeychainKind::External, first_ext_new)?;
        }

        // Set Internal Last index after sync
        let current_int = database
            .get_last_index(KeychainKind::Internal)?
            .unwrap_or(0);
        let first_int_new = internal_max_deriv.unwrap_or(0);
        if first_int_new > current_int {
            info!("Setting internal index to {}", first_int_new);
            database.set_last_index(KeychainKind::Internal, first_int_new)?;
        }

        Ok(())
    }
}

impl ConfigurableBlockchain for CbfBlockchain {
    type Config = CBFBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, crate::Error> {
        Ok(Self::new(
            config.network,
            config.datadir.clone(),
            config.peers,
        )?)
    }
}

#[cfg(test)]
#[cfg(feature = "test-cbf")]
mod test {
    use super::*;
    use bitcoincore_rpc::RpcApi;

    crate::bdk_blockchain_tests! {
        fn test_instance(test_client: &TestClient) -> CbfBlockchain {
            // Hack : Get the middle portion of a random address string to create test specific
            // unique temp directory.
            let addrs = &test_client
                .bitcoind
                .client
                .get_new_address(None, None)
                .unwrap()
                .to_string()[10..15];

            let mut root = std::env::temp_dir();
            root = root.join(addrs);

            let bitcoind_p2p_port = test_client
            .bitcoind
            .params
            .p2p_socket
            .expect("Bitcoin P2P Port should be available");

            let config = CBFBlockchainConfig {
                network: Network::Regtest,
                datadir: Some(root),
                peers: vec![bitcoind_p2p_port.into()]
            };
            let cbf_node = CbfBlockchain::from_config(&config).unwrap();

            cbf_node
        }
    }
}
