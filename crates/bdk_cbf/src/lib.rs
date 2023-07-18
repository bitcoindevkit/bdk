use std::{net, thread};

use nakamoto::client::network::Services;
use nakamoto::client::traits::Handle as HandleTrait;
use nakamoto::client::Handle;
use nakamoto::client::{Client, Config, Error, chan, Event};
use nakamoto::common::block::Height;
use nakamoto::net::poll;

use bdk_chain::{bitcoin::{ Script, Transaction }, BlockId, ChainOracle};

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

pub struct CBFClient {
    handle: Handle<poll::reactor::Waker>,
}

pub enum CBFUpdate {
    Synced { height: Height, tip: Height },
    BlockMatched {
        transactions: Vec<Transaction>,
        block: BlockId
    },
    BlockDisconnected { block: BlockId },
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

    pub fn watch_events(&self) -> Result<CBFUpdate, Error> {
        let events_chan = self.handle.events();
        loop {
            chan::select! {
                recv(events_chan) -> event => {
                    let event = event?;
                    match event {
                        Event::Ready { .. } => {
                            todo!("Handle ready event");
                        }
                        Event::FilterProcessed { .. } => {
                            todo!("Handle filter processed event");
                        }
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

    // Create a method that takes in a CBFUpdate and turns it into a 
    // Indexed Graph update.
}
