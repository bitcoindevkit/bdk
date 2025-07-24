//! Compact block filters sync over RPC. For more details refer to [BIP157][0].
//!
//! This module is home to [`FilterIter`], a structure that returns bitcoin blocks by matching
//! a list of script pubkeys against a [BIP158][1] [`BlockFilter`].
//!
//! [0]: https://github.com/bitcoin/bips/blob/master/bip-0157.mediawiki
//! [1]: https://github.com/bitcoin/bips/blob/master/bip-0158.mediawiki

use bdk_core::bitcoin;
use bdk_core::{BlockId, CheckPoint};
use bitcoin::{bip158::BlockFilter, Block, ScriptBuf};
use bitcoincore_rpc;
use bitcoincore_rpc::{json::GetBlockHeaderResult, RpcApi};

/// Type that returns Bitcoin blocks by matching a list of script pubkeys (SPKs) against a
/// [`bip158::BlockFilter`].
#[derive(Debug)]
pub struct FilterIter<'a> {
    /// RPC client
    client: &'a bitcoincore_rpc::Client,
    /// SPK inventory
    spks: Vec<ScriptBuf>,
    /// checkpoint
    cp: CheckPoint,
    /// Header info, contains the prev and next hashes for each header.
    header: Option<GetBlockHeaderResult>,
}

impl<'a> FilterIter<'a> {
    /// Construct [`FilterIter`] with checkpoint, RPC client and SPKs.
    pub fn new(
        client: &'a bitcoincore_rpc::Client,
        cp: CheckPoint,
        spks: impl IntoIterator<Item = ScriptBuf>,
    ) -> Self {
        Self {
            client,
            spks: spks.into_iter().collect(),
            cp,
            header: None,
        }
    }

    /// Find the agreement height with the remote node and return the corresponding
    /// header info.
    ///
    /// Error if no agreement height is found.
    fn find_base(&self) -> Result<GetBlockHeaderResult, Error> {
        for cp in self.cp.iter() {
            let height = cp.height();

            let fetched_hash = self.client.get_block_hash(height as u64)?;

            if fetched_hash == cp.hash() {
                return Ok(self.client.get_block_header_info(&fetched_hash)?);
            }
        }

        Err(Error::ReorgDepthExceeded)
    }
}

/// Kind of event produced by `FilterIter`.
#[derive(Debug, Clone)]
pub enum Event {
    /// Block
    Block {
        /// checkpoint
        cp: CheckPoint,
        /// block
        block: Block,
    },
    /// No match
    NoMatch {
        /// block id
        id: BlockId,
    },
    /// Tip
    Tip {
        /// checkpoint
        cp: CheckPoint,
    },
}

impl Event {
    /// Whether this event contains a matching block.
    pub fn is_match(&self) -> bool {
        matches!(self, Event::Block { .. })
    }

    /// Return the height of the event.
    pub fn height(&self) -> u32 {
        match self {
            Self::Block { cp, .. } | Self::Tip { cp } => cp.height(),
            Self::NoMatch { id } => id.height,
        }
    }
}

impl Iterator for FilterIter<'_> {
    type Item = Result<Event, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        (|| -> Result<Option<_>, Error> {
            let mut cp = self.cp.clone();

            let header = match self.header.take() {
                Some(header) => header,
                None => {
                    // If no header is cached we need to locate a base of the local
                    // checkpoint from which the scan may proceed.
                    let header = self.find_base()?;
                    let height: u32 = header.height.try_into()?;
                    cp = cp.range(..=height).next().expect("we found a base");

                    header
                }
            };

            let mut next_hash = match header.next_block_hash {
                Some(hash) => hash,
                None => return Ok(None),
            };

            let mut next_header = self.client.get_block_header_info(&next_hash)?;

            // In case of a reorg, rewind by fetching headers of previous hashes until we find
            // one with enough confirmations.
            let mut reorg_ct: i32 = 0;
            while next_header.confirmations < 0 {
                let prev_hash = next_header
                    .previous_block_hash
                    .ok_or(Error::ReorgDepthExceeded)?;
                let prev_header = self.client.get_block_header_info(&prev_hash)?;
                next_header = prev_header;
                reorg_ct += 1;
            }

            next_hash = next_header.hash;
            let next_height: u32 = next_header.height.try_into()?;

            // Purge any no longer valid checkpoints.
            if reorg_ct.is_positive() {
                cp = cp
                    .range(..=next_height)
                    .next()
                    .ok_or(Error::ReorgDepthExceeded)?;
            }
            let block_id = BlockId {
                height: next_height,
                hash: next_hash,
            };
            let filter_bytes = self.client.get_block_filter(&next_hash)?.filter;
            let filter = BlockFilter::new(&filter_bytes);

            let next_event = if filter
                .match_any(&next_hash, self.spks.iter().map(ScriptBuf::as_ref))
                .map_err(Error::Bip158)?
            {
                let block = self.client.get_block(&next_hash)?;
                cp = cp.insert(block_id);

                Ok(Some(Event::Block {
                    cp: cp.clone(),
                    block,
                }))
            } else if next_header.next_block_hash.is_none() {
                cp = cp.insert(block_id);

                Ok(Some(Event::Tip { cp: cp.clone() }))
            } else {
                Ok(Some(Event::NoMatch { id: block_id }))
            };

            // Store the next header
            self.header = Some(next_header);
            // Update self.cp
            self.cp = cp;

            next_event
        })()
        .transpose()
    }
}

/// Error that may be thrown by [`FilterIter`].
#[derive(Debug)]
pub enum Error {
    /// RPC error
    Rpc(bitcoincore_rpc::Error),
    /// `bitcoin::bip158` error
    Bip158(bitcoin::bip158::Error),
    /// Max reorg depth exceeded.
    ReorgDepthExceeded,
    /// Error converting an integer
    TryFromInt(core::num::TryFromIntError),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(e) => write!(f, "{e}"),
            Self::Bip158(e) => write!(f, "{e}"),
            Self::ReorgDepthExceeded => write!(f, "maximum reorg depth exceeded"),
            Self::TryFromInt(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

impl From<bitcoincore_rpc::Error> for Error {
    fn from(e: bitcoincore_rpc::Error) -> Self {
        Self::Rpc(e)
    }
}

impl From<core::num::TryFromIntError> for Error {
    fn from(e: core::num::TryFromIntError) -> Self {
        Self::TryFromInt(e)
    }
}
