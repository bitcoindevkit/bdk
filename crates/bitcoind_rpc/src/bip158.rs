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
/// [`bip158::BlockFilter`](bitcoin::bip158::BlockFilter).
///
/// * `FilterIter` talks to bitcoind via JSON-RPC interface, which is handled by the
///   [`bitcoincore_rpc::Client`].
/// * Collect the script pubkeys (SPKs) you want to watch. These will usually correspond to wallet
///   addresses that have been handed out for receiving payments.
/// * Construct `FilterIter` with the RPC client, SPKs, and [`CheckPoint`]. The checkpoint tip
///   informs `FilterIter` of the height to begin scanning from. An error is thrown if `FilterIter`
///   is unable to find a common ancestor with the remote node.
/// * Scan blocks by calling `next` in a loop and processing the [`Event`]s. If a filter matched any
///   of the watched scripts, then the relevant [`Block`] is returned. Note that false positives may
///   occur. `FilterIter` will continue to yield events until it reaches the latest chain tip.
///   Events contain the updated checkpoint `cp` which may be incorporated into the local chain
///   state to stay in sync with the tip.
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

    /// Return the agreement header with the remote node.
    ///
    /// Error if no agreement header is found.
    fn find_base(&self) -> Result<GetBlockHeaderResult, Error> {
        for cp in self.cp.iter() {
            match self.client.get_block_header_info(&cp.hash()) {
                Err(e) if is_not_found(&e) => continue,
                Ok(header) if header.confirmations <= 0 => continue,
                Ok(header) => return Ok(header),
                Err(e) => return Err(Error::Rpc(e)),
            }
        }
        Err(Error::ReorgDepthExceeded)
    }
}

/// Event returned by [`FilterIter`].
#[derive(Debug, Clone)]
pub struct Event {
    /// Checkpoint
    pub cp: CheckPoint,
    /// Block, will be `Some(..)` for matching blocks
    pub block: Option<Block>,
}

impl Event {
    /// Whether this event contains a matching block.
    pub fn is_match(&self) -> bool {
        self.block.is_some()
    }

    /// Return the height of the event.
    pub fn height(&self) -> u32 {
        self.cp.height()
    }
}

impl Iterator for FilterIter<'_> {
    type Item = Result<Event, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        (|| -> Result<Option<_>, Error> {
            let mut cp = self.cp.clone();

            let header = match self.header.take() {
                Some(header) => header,
                // If no header is cached we need to locate a base of the local
                // checkpoint from which the scan may proceed.
                None => self.find_base()?,
            };

            let mut next_hash = match header.next_block_hash {
                Some(hash) => hash,
                None => return Ok(None),
            };

            let mut next_header = self.client.get_block_header_info(&next_hash)?;

            // In case of a reorg, rewind by fetching headers of previous hashes until we find
            // one with enough confirmations.
            while next_header.confirmations < 0 {
                let prev_hash = next_header
                    .previous_block_hash
                    .ok_or(Error::ReorgDepthExceeded)?;
                let prev_header = self.client.get_block_header_info(&prev_hash)?;
                next_header = prev_header;
            }

            next_hash = next_header.hash;
            let next_height: u32 = next_header.height.try_into()?;

            cp = cp.insert(BlockId {
                height: next_height,
                hash: next_hash,
            });

            let mut block = None;
            let filter =
                BlockFilter::new(self.client.get_block_filter(&next_hash)?.filter.as_slice());
            if filter
                .match_any(&next_hash, self.spks.iter().map(ScriptBuf::as_ref))
                .map_err(Error::Bip158)?
            {
                block = Some(self.client.get_block(&next_hash)?);
            }

            // Store the next header
            self.header = Some(next_header);
            // Update self.cp
            self.cp = cp.clone();

            Ok(Some(Event { cp, block }))
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

/// Whether the RPC error is a "not found" error (code: `-5`).
fn is_not_found(e: &bitcoincore_rpc::Error) -> bool {
    matches!(
        e,
        bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(e))
        if e.code == -5
    )
}
