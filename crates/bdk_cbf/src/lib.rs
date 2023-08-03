use std::{net, thread};

use bdk_chain::keychain::DerivationAdditions;
use nakamoto::client::network::Services;
use nakamoto::client::Handle;
use nakamoto::client::traits::Handle as HandleTrait;
use nakamoto::client::{chan, Client, Config, Error, Event};
use nakamoto::common::block::Height;
use nakamoto::net::poll;

pub use nakamoto::client::network::Network;

use bdk_chain::{
    bitcoin::{Script, Transaction},
    collections::BTreeMap,
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph, Indexer},
    keychain::KeychainTxOutIndex,
    BlockId, ChainOracle, ConfirmationHeightAnchor, TxGraph,
};

use core::fmt::Debug;

type Reactor = poll::Reactor<net::TcpStream>;

impl ChainOracle for CBFClient {
    type Error = nakamoto::client::Error;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        chain_tip: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        if block.height > chain_tip.height {
            return Ok(None);
        }

        Ok(
            match (
                self.handle.get_block_by_height(block.height as _)?,
                self.handle.get_block_by_height(chain_tip.height as _)?,
            ) {
                (Some(b), Some(c)) => {
                    Some(b.block_hash() == block.hash && c.block_hash() == chain_tip.hash)
                }
                _ => None,
            },
        )
    }

    fn get_chain_tip(&self) -> Result<Option<BlockId>, Self::Error> {
        let (height, header) = self.handle.get_tip()?;
        Ok(Some(BlockId {
            height: height as u32,
            hash: header.block_hash(),
        }))
    }
}

#[derive(Clone)]
pub struct CBFClient {
    handle: Handle<poll::reactor::Waker>,
}

#[derive(Debug, Clone)]
pub enum CBFUpdate {
    Synced {
        height: Height,
        tip: Height,
    },
    BlockMatched {
        transactions: Vec<Transaction>,
        block: BlockId,
    },
    BlockDisconnected {
        block: BlockId,
    },
}

pub struct CBFUpdateIterator {
    client: CBFClient,
}

impl Iterator for CBFUpdateIterator {
    type Item = Result<CBFUpdate, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.client.watch_events() {
            Ok(update) => {
                match update {
                    CBFUpdate::Synced { height, tip } if height == tip => None,
                    _ => Some(Ok(update))
                }
            }
            Err(e) => Some(Err(e)),
        }
    }
}

impl CBFClient {
    pub fn start_client(cfg: Config, peer_count: usize) -> Result<Self, Error> {
        let client = Client::<Reactor>::new()?;
        let handle = client.handle();

        // Run the client on a different thread, to not block the main thread.
        thread::spawn(|| client.run(cfg).unwrap());

        // Wait for the client to be connected to a peer.
        handle.wait_for_peers(peer_count, Services::default())?;

        println!("Connected to {} peers", peer_count);

        Ok(Self { handle })
    }

    //create a function to watch
    pub fn start_scanning(
        &self,
        start_height: Height,
        watch: impl Iterator<Item = Script>,
    ) -> Result<(), Error> {
        self.handle.rescan(start_height.., watch)?;
        println!("About to start scanning from height {}", start_height);
        Ok(())
    }

    // Watch for Block events that match the scripts we're interested in
    pub fn watch_events(&self) -> Result<CBFUpdate, Error> {
        let events_chan = self.handle.events();
        loop {
            print!("looping...");
            chan::select! {
                recv(events_chan) -> event => {
                    let event = event?;
                    match event {
                        Event::BlockDisconnected { hash, height, .. } => {
                            return Ok(CBFUpdate::BlockDisconnected { block: BlockId { height: height as u32, hash } });
                        }
                        Event::BlockMatched {
                            hash,
                            height,
                            transactions,
                            ..
                        } => {
                            println!("Block matched: {} {}", height, hash);
                            return Ok(CBFUpdate::BlockMatched {
                                transactions,
                                block: BlockId { height: height as u32, hash }
                            });
                        }
                        Event::Synced { height, tip } => {
                            println!("Synced: {} {}", height, tip);
                            return Ok(CBFUpdate::Synced { height, tip });
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Turns a CBFUpdate into a TxGraph update
    pub fn into_tx_graph_update(
        &self,
        block_txs: Vec<(BlockId, Vec<Transaction>)>,
    ) -> TxGraph<ConfirmationHeightAnchor> {
        let mut tx_graph = TxGraph::default();

        for (blockid, txs) in block_txs.into_iter() {
            for tx in txs {
                let txid = tx.txid();
                let _ = tx_graph.insert_anchor(txid, to_confirmation_height_anchor(blockid));
                let _ = tx_graph.insert_tx(tx);
            }
        }
        tx_graph
    }

    pub fn iter(&self) -> CBFUpdateIterator {
        CBFUpdateIterator {
            client: self.clone(),
        }
    }

    pub fn scan<K>(
        &self,
        mut watch_per_keychain: u32,
        start_height: Height,
        indexed_tx_graph: &mut IndexedTxGraph<ConfirmationHeightAnchor, KeychainTxOutIndex<K>>,
        stop_gap: u32,
    ) -> Result<IndexedAdditions<ConfirmationHeightAnchor, DerivationAdditions<K>>, Error>
    where
        K: Ord + Clone + Debug,
    {
        let mut keychain_spks = indexed_tx_graph.index.spks_of_all_keychains();
        let mut empty_scripts_counter = BTreeMap::<K, u32>::new();
        keychain_spks.keys().for_each(|k| {
            empty_scripts_counter.insert(k.clone(), 0);
        });

        let mut updates = Vec::new();

        while let Some(keychains) = Self::check_stop_gap(stop_gap, &empty_scripts_counter) {
            keychains.iter().for_each(|k| {
                /*let (_, _) =*/ indexed_tx_graph.index.set_lookahead(k, watch_per_keychain);
            });

            let mut spk_watchlist = BTreeMap::<K, Vec<Script>>::new();
            for (k, script_iter) in keychain_spks.iter_mut() {
                (0..watch_per_keychain).for_each(|_| {
                    if let Some((_, script)) = script_iter.next() {
                        let spks = spk_watchlist.entry(k.clone()).or_insert(vec![]);
                        spks.push(script);
                    }
                });
            }

            let scripts = spk_watchlist.values().flatten().cloned().collect::<Vec<_>>();
            self.start_scanning(start_height, scripts.into_iter())?;

            for update in self.iter() {
                match update {
                    Ok(CBFUpdate::BlockMatched {
                        transactions,
                        block,
                    }) => {
                        let relevant_txs = transactions
                            .into_iter()
                            .filter(|tx| indexed_tx_graph.index.is_tx_relevant(tx))
                            .collect::<Vec<_>>();
                        updates.push((block, relevant_txs));
                    }
                    Ok(CBFUpdate::BlockDisconnected { .. }) => {
                        //TODO: Don't know how to handle re-orgs yet
                        //I will love to get your comments on this.
                    }
                    Ok(_) => {}
                    Err(e) => {
                        return Err(e);
                    }
                }
            }

            // Determine which scripts are part of the update.
            for (k, scripts) in spk_watchlist.iter() {
                for script in scripts {
                    let counter = empty_scripts_counter.get_mut(k).unwrap();
                    if Self::is_script_in_udpate(script.clone(), &updates) {
                        *counter = 0;
                    } else {
                        *counter += 1;
                    }
                }
            }

            watch_per_keychain += watch_per_keychain;
        }

        //apply the updates to IndexedGraph
        let graph_update = self.into_tx_graph_update(updates);
        let additions = indexed_tx_graph.apply_update(graph_update);

        Ok(additions)
    }

    fn is_script_in_udpate(script: Script, updates: &Vec<(BlockId, Vec<Transaction>)>) -> bool {
        for update in updates {
            for tx in update.1.iter() {
                for output in tx.output.iter() {
                    if output.script_pubkey == script {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn check_stop_gap<K>(stop_gap: u32, empty_scripts_counter: &BTreeMap<K, u32>) -> Option<Vec<K>>
    where
        K: Ord + Clone + Debug,
    {
        let keychains = empty_scripts_counter
            .iter()
            .filter(|(_, counter)| **counter < stop_gap)
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        if keychains.is_empty() {
            None
        } else {
            Some(keychains)
        }
    }

    pub fn submit_transaction(&self, tx: Transaction) -> Result<(), Error> {
        self.handle.submit_transaction(tx)?;
        Ok(())
    }

    pub fn shutdown(self) -> Result<(), Error>{
        self.handle.shutdown()?;
        Ok(())
    }
}

fn to_confirmation_height_anchor(blockid: BlockId) -> ConfirmationHeightAnchor {
    ConfirmationHeightAnchor {
        anchor_block: blockid,
        confirmation_height: blockid.height,
    }
}
