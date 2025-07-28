//! Compact block filters sync over RPC. For more details refer to [BIP157][0].
//!
//! This module is home to [`FilterIter`], a structure that returns bitcoin blocks by matching
//! a list of script pubkeys against a [BIP158][1] [`BlockFilter`].
//!
//! [0]: https://github.com/bitcoin/bips/blob/master/bip-0157.mediawiki
//! [1]: https://github.com/bitcoin/bips/blob/master/bip-0158.mediawiki

use bdk_core::collections::{BTreeMap, BTreeSet};
use core::fmt;

use bdk_core::bitcoin;
use bdk_core::{BlockId, CheckPoint};
use bitcoin::{
    bip158::{self, BlockFilter},
    block::Header,
    Block, BlockHash, ScriptBuf,
};
use bitcoincore_rpc;
use bitcoincore_rpc::RpcApi;

/// Block height
type Height = u32;

/// Block header with associated block hash.
type HashedHeader = (BlockHash, Header);

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
    // block headers
    headers: BTreeMap<Height, HashedHeader>,
    // heights of matching blocks
    matched: BTreeSet<Height>,
    // best height counter
    height: Height,
    // initial height
    start: Height,
    // stop height
    stop: Height,
}

impl<'c, C: RpcApi> FilterIter<'c, C> {
    /// Hard cap on how far to walk back when a reorg is detected.
    const MAX_REORG_DEPTH: u32 = 100;
    /// Number of recent blocks from the tip to be returned in a chain update.
    const CHAIN_SUFFIX_LEN: u32 = 10;

    /// Construct [`FilterIter`] from a given `client` and start `height`.
    pub fn new_with_height(client: &'c C, height: u32) -> Self {
        Self {
            client,
            spks: vec![],
            cp: None,
            headers: BTreeMap::new(),
            matched: BTreeSet::new(),
            height,
            start: height,
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

    /// Get the remote tip.
    ///
    /// Returns `None` if the remote height is less than the height of this [`FilterIter`].
    pub fn get_tip(&mut self) -> Result<Option<BlockId>, Error> {
        let tip_hash = self.client.get_best_block_hash()?;
        let header = self.client.get_block_header_info(&tip_hash)?;
        let tip_height = header.height as u32;
        if self.height > tip_height {
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

    /// Return all of the block headers that were collected during the scan.
    pub fn block_headers(&self) -> &BTreeMap<Height, (BlockHash, Header)> {
        &self.headers
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
            // Fetch next header.
            let mut height = self.height;
            let mut hash = self.client.get_block_hash(height as u64)?;

            let mut reorg_depth = 0;

            let header = loop {
                if reorg_depth >= Self::MAX_REORG_DEPTH {
                    return Err(Error::ReorgDepthExceeded);
                }

                let header = self.client.get_block_header(&hash)?;

                let prev_height = height.saturating_sub(1);
                match self.headers.get(&prev_height).copied() {
                    // Not enough data.
                    None => break header,
                    // Ok, the chain is consistent.
                    Some((prev_hash, _)) if prev_hash == header.prev_blockhash => break header,
                    _ => {
                        // Reorg detected, keep backtracking.
                        height = height.saturating_sub(1);
                        hash = self.client.get_block_hash(height as u64)?;
                        reorg_depth += 1;
                    }
                }
            };

            let filter_bytes = self.client.get_block_filter(&hash)?.filter;
            let filter = BlockFilter::new(&filter_bytes);

            // If the filter matches any of our watched SPKs, fetch the full
            // block and prepare the next event.
            let next_event = if self.spks.is_empty() {
                Err(Error::NoScripts)
            } else if filter
                .match_any(&hash, self.spks.iter().map(|s| s.as_bytes()))
                .map_err(Error::Bip158)?
            {
                let block = self.client.get_block(&hash)?;
                let inner = EventInner { height, block };
                Ok(Some(Event::Block(inner)))
            } else {
                Ok(Some(Event::NoMatch(height)))
            };

            // In case of a reorg, throw out any stale entries.
            if reorg_depth > 0 {
                self.headers.split_off(&height);
                self.matched.split_off(&height);
            }
            // Record the scanned block
            self.headers.insert(height, (hash, header));
            // Record the matching block
            if let Ok(Some(Event::Block(..))) = next_event {
                self.matched.insert(height);
            }
            // Increment next height
            self.height = height.saturating_add(1);

            next_event
        })()
        .transpose()
    }
}

impl<C: RpcApi> FilterIter<'_, C> {
    /// Returns the point of agreement between `self` and the given `cp`.
    fn find_base_with(&mut self, mut cp: CheckPoint) -> Result<BlockId, Error> {
        loop {
            let height = cp.height();
            let (fetched_hash, header) = match self.headers.get(&height).copied() {
                Some(value) => value,
                None => {
                    let hash = self.client.get_block_hash(height as u64)?;
                    let header = self.client.get_block_header(&hash)?;
                    (hash, header)
                }
            };
            if cp.hash() == fetched_hash {
                // Ensure this block also exists in `self`.
                self.headers.insert(height, (fetched_hash, header));
                return Ok(cp.block_id());
            }
            // Remember conflicts.
            self.headers.insert(height, (fetched_hash, header));
            cp = cp.prev().ok_or(Error::ReorgDepthExceeded)?;
        }
    }

    /// Returns a chain update from the newly scanned blocks.
    ///
    /// Returns `None` if this [`FilterIter`] was not constructed using a [`CheckPoint`], or
    /// if not all events have been emitted (by calling `next`).
    pub fn chain_update(&self) -> Option<CheckPoint> {
        if self.cp.is_none() || self.headers.is_empty() || self.height <= self.stop {
            return None;
        }

        // We return blocks up to and including the initial height, all of the matching blocks,
        // and blocks in the terminal range.
        let tail_range = (self.stop + 1).saturating_sub(Self::CHAIN_SUFFIX_LEN)..=self.stop;
        Some(
            CheckPoint::from_block_ids(self.headers.iter().filter_map(|(&height, &(hash, _))| {
                if height <= self.start
                    || self.matched.contains(&height)
                    || tail_range.contains(&height)
                {
                    Some(BlockId { height, hash })
                } else {
                    None
                }
            }))
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
    /// `MAX_REORG_DEPTH` exceeded
    ReorgDepthExceeded,
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
            Self::ReorgDepthExceeded => write!(f, "maximum reorg depth exceeded"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[cfg(test)]
mod test {
    use super::*;

    use bdk_testenv::{anyhow, bitcoind, TestEnv};
    use bitcoin::{Address, Amount, Network, ScriptBuf};
    use bitcoincore_rpc::RpcApi;

    fn testenv() -> anyhow::Result<TestEnv> {
        let mut conf = bitcoind::Conf::default();
        conf.args.push("-blockfilterindex=1");
        conf.args.push("-peerblockfilters=1");
        TestEnv::new_with_config(bdk_testenv::Config {
            bitcoind: conf,
            ..Default::default()
        })
    }

    #[test]
    fn filter_iter_matches_blocks() -> anyhow::Result<()> {
        let env = testenv()?;
        let addr = env
            .rpc_client()
            .get_new_address(None, None)?
            .assume_checked();

        let _ = env.mine_blocks(100, Some(addr.clone()))?;
        assert_eq!(env.rpc_client().get_block_count()?, 101);

        // Send tx to external address to confirm at height = 102
        let _txid = env.send(
            &Address::from_script(
                &ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?,
                Network::Regtest,
            )?,
            Amount::from_btc(0.42)?,
        )?;
        let _ = env.mine_blocks(1, None);

        let mut iter = FilterIter::new_with_height(&env.bitcoind.client, 1);
        assert_eq!(iter.get_tip()?.unwrap().height, 102);

        // Iterate events with no SPKs, expect none to match.
        for res in iter.by_ref().take(3) {
            match res {
                Err(..) => {}
                Ok(event) => {
                    assert!(!event.is_match());
                }
            }
        }

        assert!(iter.matched.is_empty());

        // Now add spks
        iter.add_spk(addr.script_pubkey());

        for res in iter.by_ref() {
            let event = res?;
            match event.height() {
                h if h <= 101 => {
                    assert!(event.is_match(), "we mined blocks to `addr`");
                }
                102 => {
                    assert!(!event.is_match(), "_txid is not relevant to `addr`");
                }
                _ => unreachable!("we stopped at height 102"),
            }
        }

        // Range of matching heights [4, 101]
        assert_eq!(iter.matched, (4..=101).collect::<BTreeSet<_>>());

        Ok(())
    }
}
