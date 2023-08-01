use std::{net, thread};

use nakamoto::client::network::Services;
use nakamoto::client::traits::Handle as HandleTrait;
use nakamoto::client::Handle;
use nakamoto::client::{chan, Client, Config, Error, Event};
use nakamoto::common::block::Height;
use nakamoto::net::poll;

use bdk_chain::{
    bitcoin::{Script, Transaction},
    BlockId, ChainOracle, TxGraph,
};

/// The network reactor we're going to use.
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
                if let CBFUpdate::Synced { .. } = update {
                    None
                } else {
                    Some(Ok(update))
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

        Ok(Self { handle })
    }

    //create a function to watch
    pub fn start_scanning(
        &self,
        start_height: Height,
        watch: impl Iterator<Item = Script>,
    ) -> Result<(), Error> {
        self.handle.rescan(start_height.., watch)?;
        Ok(())
    }

    // Watch for Block events that match the scripts we're interested in
    pub fn watch_events(&self) -> Result<CBFUpdate, Error> {
        let events_chan = self.handle.events();
        loop {
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
                            return Ok(CBFUpdate::BlockMatched {
                                transactions,
                                block: BlockId { height: height as u32, hash }
                            });
                        }
                        Event::Synced { height, tip } => {
                            return Ok(CBFUpdate::Synced { height, tip });
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Turns a CBFUpdate into a TxGraph update
    pub fn into_tx_graph_update<F>(
        &self,
        txs: Vec<Transaction>,
        block: BlockId,
        is_relevant: F,
    ) -> TxGraph<BlockId>
    where
        F: Fn(&Transaction) -> bool,
    {
        let mut tx_graph = TxGraph::default();
        let filtered_txs = txs.into_iter().filter(|tx| is_relevant(tx));
        for tx in filtered_txs {
            let txid = tx.txid();
            let _ = tx_graph.insert_anchor(txid, block);
            let _ = tx_graph.insert_tx(tx);
        }
        tx_graph
    }

    pub fn iter(&self) -> CBFUpdateIterator {
        CBFUpdateIterator {
            client: self.clone(),
        }
    }
}
