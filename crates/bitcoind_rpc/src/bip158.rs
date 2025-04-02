//! Compact block filters sync over RPC. For more details refer to [BIP157][0].
//!
//! This module is home to [`FilterIter`], a structure that returns bitcoin blocks by matching
//! a list of script pubkeys against a [BIP158][1] [`BlockFilter`].
//!
//! [0]: https://github.com/bitcoin/bips/blob/master/bip-0157.mediawiki
//! [1]: https://github.com/bitcoin/bips/blob/master/bip-0158.mediawiki

use bdk_core::collections::BTreeMap;
use core::fmt;

use bdk_core::bitcoin;
use bdk_core::{BlockId, CheckPoint};
use bitcoin::{
    bip158::{self, BlockFilter},
    Block, BlockHash, ScriptBuf,
};
use bitcoincore_rpc;
use bitcoincore_rpc::RpcApi;

/// Block height
type Height = u32;

/// Type that generates block [`Event`]s by matching a list of script pubkeys against a
/// [`BlockFilter`].
#[derive(Debug)]
pub struct FilterIter<'c, C> {
    // RPC client
    client: &'c C,
    // SPK inventory
    spks: Vec<ScriptBuf>,
    // local cp
    cp: Option<CheckPoint>,
    // map of height -> (hash, is_match)
    blocks: BTreeMap<Height, (BlockHash, bool)>,
    // initial height
    start: Height,
    // best height counter
    height: Height,
    // stop height
    stop: Height,
}

impl<'c, C: RpcApi> FilterIter<'c, C> {
    /// Construct [`FilterIter`] from a given `client` and start `height`.
    pub fn new_with_height(client: &'c C, height: u32) -> Self {
        Self {
            client,
            spks: vec![],
            cp: None,
            blocks: BTreeMap::new(),
            start: height,
            height,
            stop: 0,
        }
    }

    /// Construct [`FilterIter`] from a given `client` and [`CheckPoint`].
    pub fn new_with_checkpoint(client: &'c C, cp: CheckPoint) -> Self {
        let mut filter_iter = Self::new_with_height(client, cp.height());
        filter_iter.cp = Some(cp);
        filter_iter
    }

    /// Extends `self` with an iterator of spks.
    pub fn add_spks(&mut self, spks: impl IntoIterator<Item = ScriptBuf>) {
        self.spks.extend(spks)
    }

    /// Add spk to the list of spks to scan with.
    pub fn add_spk(&mut self, spk: ScriptBuf) {
        self.spks.push(spk);
    }

    /// Get the block hash by `height` if it is found in the blocks map.
    fn get_block_hash(&self, height: &Height) -> Option<BlockHash> {
        Some(self.blocks.get(height)?.0)
    }

    /// Insert a (non-matching) block height and hash into the blocks map.
    fn insert_block(&mut self, height: Height, hash: BlockHash) {
        self.blocks.insert(height, (hash, false));
    }

    /// Get the remote tip.
    ///
    /// Returns `None` if the remote height is not strictly greater than the height of this
    /// [`FilterIter`].
    pub fn get_tip(&mut self) -> Result<Option<BlockId>, Error> {
        let tip_hash = self.client.get_best_block_hash()?;
        let header = self.client.get_block_header_info(&tip_hash)?;
        let tip_height = header.height as u32;
        if self.height >= tip_height {
            // nothing to do
            return Ok(None);
        }

        // start scanning from point of agreement + 1
        if let Some(cp) = self.cp.as_ref() {
            let base = self.find_base_with(cp.clone())?;
            self.height = base.height.saturating_add(1);
        }

        self.stop = tip_height;

        Ok(Some(BlockId {
            height: tip_height,
            hash: tip_hash,
        }))
    }
}

/// Event inner type
#[derive(Debug, Clone)]
pub struct EventInner {
    /// Height
    pub height: Height,
    /// Block
    pub block: Block,
}

/// Kind of event produced by [`FilterIter`].
#[derive(Debug, Clone)]
pub enum Event {
    /// Block
    Block(EventInner),
    /// No match
    NoMatch(Height),
}

impl Event {
    /// Whether this event contains a matching block.
    pub fn is_match(&self) -> bool {
        matches!(self, Event::Block(_))
    }

    /// Get the height of this event.
    pub fn height(&self) -> Height {
        match self {
            Self::Block(EventInner { height, .. }) => *height,
            Self::NoMatch(h) => *h,
        }
    }
}

impl<C: RpcApi> Iterator for FilterIter<'_, C> {
    type Item = Result<Event, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        (|| -> Result<Option<_>, Error> {
            if self.height > self.stop {
                return Ok(None);
            }
            // Fetch next filter
            let mut height = self.height;
            let mut hash = self.client.get_block_hash(height as _)?;
            loop {
                let header = self.client.get_block_header(&hash)?;
                let prev_height = height.saturating_sub(1);
                let prev_hash = match self.get_block_hash(&prev_height) {
                    Some(hash) => hash,
                    None => break,
                };
                if header.prev_blockhash == prev_hash {
                    break;
                }
                // reorg detected, keep backtracking
                height = height.saturating_sub(1);
                hash = self.client.get_block_hash(height as _)?;
            }
            let filter_bytes = self.client.get_block_filter(&hash)?.filter;
            let filter = BlockFilter::new(&filter_bytes);

            // record the scanned block
            self.insert_block(height, hash);
            // increment best height
            self.height = height.saturating_add(1);

            // If the filter matches any of our watched SPKs, fetch the full
            // block, and record the matching block entry.
            if self.spks.is_empty() {
                Err(Error::NoScripts)
            } else if filter
                .match_any(&hash, self.spks.iter().map(|s| s.as_bytes()))
                .map_err(Error::Bip158)?
            {
                let block = self.client.get_block(&hash)?;
                // update for matched block
                self.blocks.entry(height).and_modify(|(_, is_match)| {
                    *is_match = true;
                });
                let inner = EventInner { height, block };
                Ok(Some(Event::Block(inner)))
            } else {
                Ok(Some(Event::NoMatch(height)))
            }
        })()
        .transpose()
    }
}

impl<C: RpcApi> FilterIter<'_, C> {
    /// Returns the point of agreement between `self` and the given `cp`.
    fn find_base_with(&mut self, mut cp: CheckPoint) -> Result<BlockId, Error> {
        loop {
            let height = cp.height();
            let fetched_hash = match self.get_block_hash(&height) {
                Some(hash) => hash,
                None if height == 0 => cp.hash(),
                _ => self.client.get_block_hash(height as _)?,
            };
            if cp.hash() == fetched_hash {
                // ensure this block also exists in self
                self.insert_block(height, cp.hash());
                return Ok(cp.block_id());
            }
            // remember conflicts
            self.insert_block(height, fetched_hash);
            cp = cp.prev().expect("must break before genesis");
        }
    }

    /// Returns a chain update from the newly scanned blocks.
    ///
    /// Returns `None` if this [`FilterIter`] was not constructed using a [`CheckPoint`], or
    /// if not all events have been emitted (by calling `next`).
    pub fn chain_update(&mut self) -> Option<CheckPoint> {
        if self.cp.is_none() || self.blocks.is_empty() || self.height <= self.stop {
            return None;
        }

        // We return blocks up to and including the initial height, all of the matching blocks,
        // and blocks in the terminal range.
        let tail_range = self.stop.saturating_sub(9)..=self.stop;
        Some(
            CheckPoint::from_block_ids(self.blocks.iter().filter_map(
                |(&height, &(hash, is_match))| {
                    if height <= self.start || is_match || tail_range.contains(&height) {
                        Some(BlockId { height, hash })
                    } else {
                        None
                    }
                },
            ))
            .expect("blocks must be in order"),
        )
    }
}

/// Errors that may occur during a compact filters sync.
#[derive(Debug)]
pub enum Error {
    /// bitcoin bip158 error
    Bip158(bip158::Error),
    /// attempted to scan blocks without any script pubkeys
    NoScripts,
    /// `bitcoincore_rpc` error
    Rpc(bitcoincore_rpc::Error),
}

impl From<bitcoincore_rpc::Error> for Error {
    fn from(e: bitcoincore_rpc::Error) -> Self {
        Self::Rpc(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bip158(e) => e.fmt(f),
            Self::NoScripts => write!(f, "no script pubkeys were provided to match with"),
            Self::Rpc(e) => e.fmt(f),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}
