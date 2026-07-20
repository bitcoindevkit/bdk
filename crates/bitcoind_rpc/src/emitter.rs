use alloc::sync::Arc;
use core::fmt;

use bdk_core::collections::{HashMap, HashSet};
use bdk_core::{BlockId, CheckPoint, ToBlockHash};
use bitcoin::{block::Header, Block, BlockHash, Transaction, Txid};
use bitcoind_client::bitreq::Client;
use bitcoind_client::corepc_types::model::GetBlockVerboseOne;

/// Maximum number of consecutive failed attempts to find an agreement point before
/// [`Emitter::next_block`] returns [`EmitterError::AgreementNotFound`].
const MAX_AGREEMENT_FAILURES: u8 = 3;

/// The [`Emitter`] is used to emit data sourced from [`bitcoind_client::bitreq::Client`].
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub struct Emitter<'a, B> {
    client: &'a Client,

    /// Height from which to start emitting blocks. Defaults to `last_cp.height()`.
    start_height: u32,

    /// Checkpoint of the last-emitted block known to be in the best chain, and the tail of the
    /// linked checkpoint list threaded through each [`BlockEvent`]. Blocks reorged out are popped
    /// off during agreement scanning.
    last_cp: CheckPoint<B>,

    /// RPC result of the last-emitted block, kept for its `nextblockhash` so the next block can be
    /// fetched without a height-to-hash lookup. `None` before the first emission, at tip, or after
    /// the last block was reorged out.
    last_block: Option<GetBlockVerboseOne>,

    /// Consecutive polls that failed to find an agreement point. Bounds the retry loop so the
    /// emitter surfaces [`EmitterError::AgreementNotFound`] instead of spinning forever.
    agreement_failure_count: u8,

    /// Unconfirmed transactions seen so far. Doubles as a fetch cache (avoids re-fetching txs) and
    /// as the reference set for eviction detection: at tip, any txid here but absent from the
    /// latest `getrawmempool` is reported evicted. Entries are removed once confirmed in a block.
    mempool_snapshot: HashMap<Txid, Arc<Transaction>>,
}

impl<'a, B> Emitter<'a, B>
where
    B: ToBlockHash + Clone + fmt::Debug + From<Header>,
{
    /// Construct a new [`Emitter`].
    ///
    /// `last_cp` is the chain the emitter starts from: it scans this checkpoint chain to find the
    /// deepest block still in the node's best chain, then emits forward from there. Emission
    /// starts at `last_cp.height()` by default; use [`Emitter::start_height`] to skip ahead (for
    /// example, to a wallet's birthday).
    ///
    /// `expected_mempool_txs` is the wallet's set of already-known unconfirmed transactions, which
    /// lets the emitter report evictions for them. Pass `core::iter::empty()` when empty.
    pub fn new(
        client: &'a Client,
        last_cp: CheckPoint<B>,
        expected_mempool_txs: impl IntoIterator<Item = impl Into<Arc<Transaction>>>,
    ) -> Self {
        let start_height = last_cp.height();
        Self {
            client,
            start_height,
            last_cp,
            last_block: None,
            agreement_failure_count: 0,
            mempool_snapshot: expected_mempool_txs
                .into_iter()
                .map(|tx| {
                    let tx: Arc<Transaction> = tx.into();
                    (tx.compute_txid(), tx)
                })
                .collect(),
        }
    }

    /// Set the block height from which to start emitting blocks.
    ///
    /// By default the emitter starts from `last_cp.height()`. Use this when `last_cp` is at a low
    /// height (for example, genesis) but the wallet only needs blocks from a later point — its
    /// "birthday". During a reorg, if the agreement point is found below the local checkpoint, the
    /// emitter automatically lowers `start_height` so that no invalidated heights are skipped.
    pub fn start_height(mut self, start_height: u32) -> Self {
        self.start_height = start_height;
        self
    }

    /// Retrieve all txids currently in the mempool, ensuring the snapshot is consistent with the
    /// remote tip (the best block hash is unchanged across the call).
    fn raw_mempool(&self) -> Result<(BlockHash, Vec<Txid>), EmitterError> {
        loop {
            let block_hash = self.client.get_best_block_hash()?;
            let raw_mempool = self.client.get_raw_mempool()?;
            if self.client.get_best_block_hash()? == block_hash {
                return Ok((block_hash, raw_mempool));
            }
        }
    }

    /// Emit a full snapshot of the mempool along with any evicted [`Txid`]s.
    ///
    /// The returned [`MempoolEvent`] timestamps every entry with the current time (see
    /// [`mempool_at`](Self::mempool_at) for details on eviction reporting).
    #[cfg(feature = "std")]
    pub fn mempool(&mut self) -> Result<MempoolEvent, EmitterError> {
        let sync_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("must get current time")
            .as_secs();
        self.mempool_at(sync_time)
    }

    /// Emit a full snapshot of the mempool along with any evicted [`Txid`]s, timestamped with the
    /// given `sync_time` (unix seconds). This is the no-std version of [`mempool`](Self::mempool).
    ///
    /// Evictions are only reported once the emitter has caught up to the node's tip (its
    /// checkpoint hash matches the best block). Until then we cannot tell an evicted transaction
    /// apart from one confirmed in a not-yet-emitted block, so [`MempoolEvent::evicted`] stays
    /// empty and transactions accumulate in the snapshot to be reconciled once at tip.
    pub fn mempool_at(&mut self, sync_time: u64) -> Result<MempoolEvent, EmitterError> {
        let (tip_hash, raw_mempool) = self.raw_mempool()?;

        let mempool_txids: HashSet<Txid> = raw_mempool.iter().copied().collect();

        let mempool_txs: Vec<(Txid, Arc<Transaction>, u64)> = raw_mempool
            .into_iter()
            .filter_map(|txid| -> Option<Result<_, bitcoind_client::Error>> {
                let tx = match self.mempool_snapshot.get(&txid) {
                    Some(tx) => tx.clone(),
                    None => match self.client.get_raw_transaction(&txid) {
                        Ok(tx) => {
                            let tx = Arc::new(tx);
                            self.mempool_snapshot.insert(txid, tx.clone());
                            tx
                        }
                        Err(err) if err.is_not_found_error() => return None,
                        Err(err) => return Some(Err(err)),
                    },
                };
                Some(Ok((txid, tx, sync_time)))
            })
            .collect::<Result<_, _>>()?;

        let mut mempool_event = MempoolEvent {
            update: mempool_txs
                .iter()
                .map(|(_, tx, sync_time)| (tx.clone(), *sync_time))
                .collect(),
            ..Default::default()
        };

        let at_tip = self.last_cp.hash() == tip_hash;

        if at_tip {
            // At tip we can trust that a missing txid was evicted rather than confirmed, so report
            // evictions and replace the snapshot with the current mempool.
            mempool_event.evicted = self
                .mempool_snapshot
                .keys()
                .filter(|&txid| !mempool_txids.contains(txid))
                .map(|&txid| (txid, sync_time))
                .collect();
            self.mempool_snapshot = mempool_txs
                .iter()
                .map(|(txid, tx, _)| (*txid, tx.clone()))
                .collect();
        } else {
            // Still catching up: accumulate so evictions can be reconciled in one batch at tip.
            self.mempool_snapshot
                .extend(mempool_txs.iter().map(|(txid, tx, _)| (*txid, tx.clone())));
        };

        Ok(mempool_event)
    }

    /// Emit the next block in chain order, or `Ok(None)` once the tip is reached.
    ///
    /// Blocks are emitted consecutively from the agreement point (the deepest checkpoint still in
    /// the node's best chain). On a reorg the emitter rescans for a new agreement point and
    /// re-emits from there. Returns [`EmitterError::AgreementNotFound`] if no agreement point can
    /// be found after multiple consecutive attempts.
    pub fn next_block(&mut self) -> Result<Option<BlockEvent<B>>, EmitterError> {
        if let Some((checkpoint, block)) = self.poll()? {
            // Confirmed transactions leave the mempool snapshot so they aren't misreported as
            // evictions on the next mempool() call.
            for tx in &block.txdata {
                self.mempool_snapshot.remove(&tx.compute_txid());
            }
            return Ok(Some(BlockEvent { block, checkpoint }));
        }
        Ok(None)
    }
}

/// A new emission from mempool.
#[derive(Debug, Default)]
pub struct MempoolEvent {
    /// A full snapshot of the current mempool, each transaction paired with the
    /// `sync_time` of the call that produced it.
    pub update: Vec<(Arc<Transaction>, u64)>,

    /// Transactions evicted from the mempool since the last call, paired with the call's
    /// `sync_time`. Only populated once the emitter is at the node's tip; empty while catching up.
    pub evicted: Vec<(Txid, u64)>,
}

/// A newly emitted block from [`Emitter`].
#[derive(Debug)]
pub struct BlockEvent<B> {
    /// The block.
    pub block: Block,

    /// The checkpoint of the new block.
    ///
    /// A [`CheckPoint`] is a node of a linked list of [`BlockId`]s. This checkpoint is linked to
    /// all [`BlockId`]s originally passed in [`Emitter::new`] as well as emitted blocks since
    /// then. These blocks are guaranteed to be of the same chain.
    ///
    /// This is important as BDK structures require block-to-apply to be connected with another
    /// block in the original chain.
    pub checkpoint: CheckPoint<B>,
}

impl<B> BlockEvent<B> {
    /// The block height of this new block.
    pub fn block_height(&self) -> u32 {
        self.checkpoint.height()
    }

    /// The block hash of this new block.
    pub fn block_hash(&self) -> BlockHash {
        self.checkpoint.hash()
    }

    /// The [`BlockId`] this block's checkpoint chain connects to.
    ///
    /// This is the previous entry in the emitter's checkpoint chain. For consecutive emissions
    /// it is the Bitcoin parent block; when [`Emitter::start_height`] skips ahead, the first
    /// emission connects to whatever checkpoint was at the tail of `last_cp` (possibly an
    /// ancestor further back than the direct parent).
    pub fn connected_to(&self) -> BlockId {
        match self.checkpoint.prev() {
            Some(prev_cp) => prev_cp.block_id(),
            // No previous checkpoint; derive the parent from this block's header.
            // This should be unreachable in practice, since every emitted block connects
            // to `last_cp`.
            None => BlockId {
                height: self.checkpoint.height().saturating_sub(1),
                hash: self.block.header.prev_blockhash,
            },
        }
    }
}

/// Outcome of a single node poll, driving the [`Emitter`] state machine (see
/// [`Emitter::poll_once`]).
enum PollResponse<B> {
    /// The next consecutive block is ready to emit.
    NextBlock(GetBlockVerboseOne),
    /// The emitter is current with the node's tip; there is no next block.
    Tip,
    /// The last emitted block is no longer in the best chain; fall through to agreement scanning.
    Reorged,
    /// A checkpoint still in the best chain was found; resume scanning from here.
    AgreementAt(GetBlockVerboseOne, CheckPoint<B>),
    /// No checkpoint matches the node's best chain.
    AgreementNotFound,
}

impl<'a, B> Emitter<'a, B>
where
    B: ToBlockHash + Clone + fmt::Debug + From<Header>,
{
    /// Probe the node once and return the appropriate `PollResponse`.
    ///
    /// `last_block` determines the next phase: when `Some`, follow its `next_block_hash` to the
    /// next block (reporting `PollResponse::Reorged` if it has been reorged out); when
    /// `None`, walk `last_cp` backwards to find the nearest checkpoint still in the best chain.
    fn poll_once(&self) -> Result<PollResponse<B>, bitcoind_client::Error> {
        if let Some(last_block_info) = &self.last_block {
            // Enforce start height
            let next_hash = if last_block_info.height + 1 < self.start_height {
                // Verify the last-emitted (or agreement) block is still in the best chain before
                // jumping ahead to `start_height`.
                if self.client.get_block_hash(last_block_info.height)? != last_block_info.hash {
                    return Ok(PollResponse::Reorged);
                }
                self.client.get_block_hash(self.start_height)?
            } else {
                match last_block_info.next_block_hash {
                    None => return Ok(PollResponse::Tip),
                    Some(next_hash) => next_hash,
                }
            };

            let block_info = self.client.get_block_verbose(&next_hash)?;
            return if block_info.confirmations < 0 {
                Ok(PollResponse::Reorged)
            } else {
                Ok(PollResponse::NextBlock(block_info))
            };
        }

        for cp in self.last_cp.iter() {
            let block_info = match self.client.get_block_verbose(&cp.hash()) {
                // block not in best chain
                Ok(block_info) if block_info.confirmations < 0 => continue,
                Ok(block_info) => block_info,
                Err(e) if e.is_not_found_error() => {
                    if cp.height() > 0 {
                        continue;
                    }
                    // genesis not found; cannot form a connected update
                    break;
                }
                Err(e) => return Err(e),
            };

            return Ok(PollResponse::AgreementAt(block_info, cp));
        }

        Ok(PollResponse::AgreementNotFound)
    }

    /// Drive the state machine until a block is ready to emit (`Some`) or the tip is reached
    /// (`None`). Returns [`EmitterError::AgreementNotFound`] after [`MAX_AGREEMENT_FAILURES`]
    /// consecutive failures to find an agreement point.
    fn poll(&mut self) -> Result<Option<(CheckPoint<B>, Block)>, EmitterError> {
        loop {
            match self.poll_once()? {
                PollResponse::NextBlock(block_info) => {
                    let height = block_info.height;
                    let hash = block_info.hash;
                    let block = self.client.get_block(&hash)?;

                    let new_cp = self
                        .last_cp
                        .clone()
                        .push(height, block.header.into())
                        .expect("NextBlock height must only increase");
                    self.last_cp = new_cp.clone();
                    self.last_block = Some(block_info);
                    self.agreement_failure_count = 0;
                    return Ok(Some((new_cp, block)));
                }
                PollResponse::Tip => {
                    self.last_block = None;
                    return Ok(None);
                }
                PollResponse::Reorged => {
                    self.last_block = None;
                }
                PollResponse::AgreementAt(block_info, cp) => {
                    // When a reorg happens, the agreement point drops below `last_cp`. We lower
                    // `start_height` so the emitter revisits the invalidated heights.
                    if block_info.height < self.last_cp.height() {
                        self.start_height = block_info.height;
                    }
                    self.last_cp = cp;
                    self.last_block = Some(block_info);
                    self.agreement_failure_count = 0;
                }
                PollResponse::AgreementNotFound => {
                    self.agreement_failure_count += 1;
                    if self.agreement_failure_count >= MAX_AGREEMENT_FAILURES {
                        return Err(EmitterError::AgreementNotFound);
                    }
                    self.last_block = None;
                }
            }
        }
    }
}

/// Errors returned by [`Emitter`] methods.
#[derive(Debug)]
#[non_exhaustive]
pub enum EmitterError {
    /// An RPC call to bitcoind failed.
    Rpc(bitcoind_client::Error),
    /// The emitter exhausted all checkpoints without finding a block that is part of the node's
    /// best chain. This indicates either a catastrophic reorg that rolled back further than any
    /// known checkpoint, or an inconsistent node (e.g. a checkpoint from the wrong network, whose
    /// genesis differs from the connected node). The caller should reinitialise the [`Emitter`]
    /// with a fresh checkpoint against the correct node.
    AgreementNotFound,
}

impl fmt::Display for EmitterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmitterError::Rpc(e) => write!(f, "bitcoind RPC error: {e}"),
            EmitterError::AgreementNotFound => write!(
                f,
                "no agreement point found between local checkpoints and the node's best chain",
            ),
        }
    }
}

impl core::error::Error for EmitterError {}

impl From<bitcoind_client::Error> for EmitterError {
    fn from(e: bitcoind_client::Error) -> Self {
        EmitterError::Rpc(e)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use crate::Emitter;
    use bdk_chain::local_chain::LocalChain;
    use bdk_testenv::{anyhow, TestEnv};
    use bitcoin::{hashes::Hash, Address, Amount, ScriptBuf, Transaction, Txid, WScriptHash};
    use std::collections::HashSet;

    #[test]
    fn test_expected_mempool_txids_accumulate_and_remove() -> anyhow::Result<()> {
        let env = TestEnv::new()?;
        let (chain, _) = LocalChain::from_genesis(env.genesis_hash()?);
        let chain_tip = chain.tip();

        let rpc_client = bitcoind_client::bitreq::Client::with_auth(
            &env.bitcoind.rpc_url(),
            bitcoind_client::bitreq::Auth::CookieFile(env.bitcoind.params.cookie_file.clone()),
        )?;

        let mut emitter = Emitter::new(
            &rpc_client,
            chain_tip.clone(),
            core::iter::empty::<Transaction>(),
        );

        env.mine_blocks(100, None)?;
        while emitter.next_block()?.is_some() {}

        let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
        let addr_to_track = Address::from_script(&spk_to_track, bitcoin::Network::Regtest)?;
        let mut mempool_txids = HashSet::new();

        // Send a tx at different heights and ensure txs are accumulating in expected_mempool_txids.
        for _ in 0..10 {
            let sent_txid = env.send(&addr_to_track, Amount::from_sat(1_000))?;
            mempool_txids.insert(sent_txid);
            emitter.mempool()?;
            env.mine_blocks(1, None)?;

            for txid in &mempool_txids {
                assert!(
                    emitter.mempool_snapshot.contains_key(txid),
                    "Expected txid {txid:?} missing"
                );
            }
        }

        // Process each block and check that confirmed txids are removed from from
        // expected_mempool_txids.
        while let Some(block_event) = emitter.next_block()? {
            let confirmed_txids: HashSet<Txid> = block_event
                .block
                .txdata
                .iter()
                .map(|tx| tx.compute_txid())
                .collect();
            mempool_txids = mempool_txids
                .difference(&confirmed_txids)
                .copied()
                .collect::<HashSet<_>>();
            for txid in confirmed_txids {
                assert!(
                    !emitter.mempool_snapshot.contains_key(&txid),
                    "Expected txid {txid:?} should have been removed"
                );
            }
            for txid in &mempool_txids {
                assert!(
                    emitter.mempool_snapshot.contains_key(txid),
                    "Expected txid {txid:?} missing"
                );
            }
        }

        assert!(emitter.mempool_snapshot.is_empty());

        Ok(())
    }
}
