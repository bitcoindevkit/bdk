use alloc::sync::Arc;
use core::fmt;

use bdk_core::collections::{HashMap, HashSet};
use bdk_core::{BlockId, CheckPoint};
use bitcoin::{Block, BlockHash, Transaction, Txid};
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
pub struct Emitter<'a> {
    client: &'a Client,
    start_height: u32,

    /// The checkpoint of the last-emitted block that is in the best chain. If it is later found
    /// that the block is no longer in the best chain, it will be popped off from here.
    last_cp: CheckPoint<BlockHash>,

    /// The block result returned from rpc of the last-emitted block. As this result contains the
    /// next block's block hash (which we use to fetch the next block), we set this to `None`
    /// whenever there are no more blocks, or the next block is no longer in the best chain. This
    /// gives us an opportunity to re-fetch this result.
    last_block: Option<GetBlockVerboseOne>,

    /// Number of consecutive times polling has failed to find an agreement point. Used to bound
    /// the retry loop and surface [`EmitterError::AgreementNotFound`] rather than spinning
    /// forever.
    agreement_failure_count: u8,

    /// The last snapshot of mempool transactions.
    ///
    /// This is used to detect mempool evictions and as a cache for transactions to emit.
    ///
    /// For mempool evictions, the latest call to `getrawmempool` is compared against this field.
    /// Any transaction that is missing from this field is considered evicted. The exception is if
    /// the transaction is confirmed into a block - therefore, we only emit evictions when we are
    /// sure the tip block is already emitted. When a block is emitted, the transactions in the
    /// block are removed from this field.
    mempool_snapshot: HashMap<Txid, Arc<Transaction>>,
}

impl<'a> Emitter<'a> {
    /// Construct a new [`Emitter`].
    ///
    /// `last_cp` informs the emitter of the chain we are starting off with. This way, the emitter
    /// can start emission from a block that connects to the original chain.
    ///
    /// By default emission starts from `last_cp.height()`. Use [`Emitter::start_height`] to start
    /// from a later height (for example, a wallet's birthday).
    ///
    /// `expected_mempool_txs` is the initial set of unconfirmed transactions provided by the
    /// wallet. This allows the [`Emitter`] to inform the wallet about relevant mempool evictions.
    /// If it is known that the wallet is empty, [`NO_EXPECTED_MEMPOOL_TXS`] can be used.
    pub fn new(
        client: &'a Client,
        last_cp: CheckPoint<BlockHash>,
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

    /// Emit mempool transactions and any evicted [`Txid`]s.
    ///
    /// This method returns a [`MempoolEvent`] containing the full transactions (with their
    /// first-seen unix timestamps) that were emitted, and [`MempoolEvent::evicted`] which are
    /// any [`Txid`]s which were previously seen in the mempool and are now missing. Evicted txids
    /// are only reported once the emitter’s checkpoint matches the RPC’s best block in both height
    /// and hash. Until `next_block()` advances the checkpoint to tip, `mempool()` will always
    /// return an empty `evicted` set.
    #[cfg(feature = "std")]
    pub fn mempool(&mut self) -> Result<MempoolEvent, EmitterError> {
        let sync_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("must get current time")
            .as_secs();
        self.mempool_at(sync_time)
    }

    /// Emit mempool transactions and any evicted [`Txid`]s at the given `sync_time`.
    ///
    /// `sync_time` is in unix seconds.
    ///
    /// This is the no-std version of [`mempool`](Self::mempool).
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
            // We only emit evicted transactions when we have already emitted the RPC tip. This is
            // because we cannot differentiate between transactions that are confirmed and
            // transactions that are evicted, so we rely on emitted blocks to remove
            // transactions from the `mempool_snapshot`.
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
            // Since we are still catching up to the tip (a.k.a tip has not been emitted), we
            // accumulate more transactions in `mempool_snapshot` so that we can emit evictions in
            // a batch once we catch up.
            self.mempool_snapshot
                .extend(mempool_txs.iter().map(|(txid, tx, _)| (*txid, tx.clone())));
        };

        Ok(mempool_event)
    }

    /// Emit the next block height and block (if any).
    pub fn next_block(&mut self) -> Result<Option<BlockEvent>, EmitterError> {
        if let Some((checkpoint, block)) = self.poll()? {
            // Stop tracking unconfirmed transactions that have been confirmed in this block.
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
    /// Transactions currently in the mempool alongside their seen-at timestamp.
    pub update: Vec<(Arc<Transaction>, u64)>,

    /// Transactions evicted from the mempool alongside their evicted-at timestamp.
    pub evicted: Vec<(Txid, u64)>,
}

/// A newly emitted block from [`Emitter`].
#[derive(Debug)]
pub struct BlockEvent {
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
    pub checkpoint: CheckPoint<BlockHash>,
}

impl BlockEvent {
    /// The block height of this new block.
    pub fn block_height(&self) -> u32 {
        self.checkpoint.height()
    }

    /// The block hash of this new block.
    pub fn block_hash(&self) -> BlockHash {
        self.checkpoint.hash()
    }

    /// The [`BlockId`] of a previous block that this block connects to.
    ///
    /// This either returns a [`BlockId`] of a previously emitted block or from the chain we started
    /// with (passed in as `last_cp` in [`Emitter::new`]).
    ///
    /// This value is derived from [`BlockEvent::checkpoint`].
    pub fn connected_to(&self) -> BlockId {
        match self.checkpoint.prev() {
            Some(prev_cp) => prev_cp.block_id(),
            // No previous checkpoint (e.g. the first emission after a `start_height` skip); derive
            // the parent from this block's header instead of connecting the block to itself.
            None => BlockId {
                height: self.checkpoint.height().saturating_sub(1),
                hash: self.block.header.prev_blockhash,
            },
        }
    }
}

/// Internal state machine responses from [`Emitter::poll_once`].
enum PollResponse {
    /// The next consecutive block is ready to be emitted.
    NextBlock(GetBlockVerboseOne),
    /// The emitter is current with the node's tip; no next block exists.
    Tip,
    /// The last emitted block is no longer in the best chain.
    Reorged,
    /// A checkpoint still in the best chain was found; resume scanning from here.
    AgreementAt(GetBlockVerboseOne, CheckPoint<BlockHash>),
    /// No checkpoint matches the node's best chain.
    AgreementNotFound,
}

impl<'a> Emitter<'a> {
    /// Probe the node once and return the appropriate [`PollResponse`].
    fn poll_once(&self) -> Result<PollResponse, bitcoind_client::Error> {
        if let Some(last_block_info) = &self.last_block {
            let next_hash = if last_block_info.height + 1 < self.start_height {
                // enforce start height
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
                    // if we can't find genesis block, we can't create an update that connects
                    break;
                }
                Err(e) => return Err(e),
            };

            // agreement point found
            return Ok(PollResponse::AgreementAt(block_info, cp));
        }

        Ok(PollResponse::AgreementNotFound)
    }

    /// Drive the emitter state machine forward until a block is ready to emit or the tip is
    /// reached.
    fn poll(&mut self) -> Result<Option<(CheckPoint<BlockHash>, Block)>, EmitterError> {
        loop {
            match self.poll_once()? {
                PollResponse::NextBlock(block_info) => {
                    let height = block_info.height;
                    let hash = block_info.hash;
                    let block = self.client.get_block(&hash)?;

                    let new_cp = self
                        .last_cp
                        .clone()
                        .push(height, hash)
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
