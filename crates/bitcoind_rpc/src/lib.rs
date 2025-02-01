//! This crate is used for emitting blockchain data from the `bitcoind` RPC interface. It does not
//! use the wallet RPC API, so this crate can be used with wallet-disabled Bitcoin Core nodes.
//!
//! [`Emitter`] is the main structure which sources blockchain data from [`bitcoincore_rpc::Client`].
//!
//! To only get block updates (exclude mempool transactions), the caller can use
//! [`Emitter::next_block`] or/and [`Emitter::next_header`] until it returns `Ok(None)` (which means
//! the chain tip is reached). A separate method, [`Emitter::mempool`] can be used to emit the whole
//! mempool.
#![warn(missing_docs)]

use bdk_core::{BlockId, CheckPoint};
use bitcoin::{block::Header, Block, BlockHash, Transaction};
use bitcoincore_rpc::bitcoincore_rpc_json;

pub mod bip158;

pub use bitcoincore_rpc;

/// The [`Emitter`] is used to emit data sourced from [`bitcoincore_rpc::Client`].
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub struct Emitter<'c, C> {
    client: &'c C,
    start_height: u32,

    /// The checkpoint of the last-emitted block that is in the best chain. If it is later found
    /// that the block is no longer in the best chain, it will be popped off from here.
    last_cp: CheckPoint,

    /// The block result returned from rpc of the last-emitted block. As this result contains the
    /// next block's block hash (which we use to fetch the next block), we set this to `None`
    /// whenever there are no more blocks, or the next block is no longer in the best chain. This
    /// gives us an opportunity to re-fetch this result.
    last_block: Option<bitcoincore_rpc_json::GetBlockResult>,

    /// The latest first-seen epoch of emitted mempool transactions. This is used to determine
    /// whether a mempool transaction is already emitted.
    last_mempool_time: usize,

    /// The last emitted block during our last mempool emission. This is used to determine whether
    /// there has been a reorg since our last mempool emission.
    last_mempool_tip: Option<u32>,

    /// Unconfirmed txids that are expected to appear in mempool. This is used to determine if any
    /// known txids have been evicted.
    expected_mempool_txids: HashSet<Txid>,
}

impl<'c, C: bitcoincore_rpc::RpcApi> Emitter<'c, C> {
    /// Construct a new [`Emitter`].
    ///
    /// `last_cp` informs the emitter of the chain we are starting off with. This way, the emitter
    /// can start emission from a block that connects to the original chain.
    ///
    /// `start_height` starts emission from a given height (if there are no conflicts with the
    /// original chain).
    ///
    /// `expected_mempool_txids` is the initial set of unconfirmed txids. Once at tip, any that are
    /// no longer in mempool are marked evicted.
    pub fn new(
        client: &'c C,
        last_cp: CheckPoint,
        start_height: u32,
        expected_mempool_txids: HashSet<Txid>,
    ) -> Self {
        Self {
            client,
            start_height,
            last_cp,
            last_block: None,
            last_mempool_time: 0,
            last_mempool_tip: None,
        }
    }

    /// Emit mempool transactions and any evicted [`Txid`]s. Returns a `latest_update_time` which is
    /// used for setting the timestamp for evicted transactions.
    ///
    /// This method returns a [`MempoolEvent`] containing the full transactions (with their
    /// first-seen unix timestamps) that were emitted, and [`MempoolEvent::evicted_txids`] which are
    /// any [`Txid`]s which were previously expected and are now missing from the mempool. Note that
    /// [`Txid`]s are only evicted if the emitter is at the chain tip with the same height and hash
    /// as the best block from the RPC.
    ///
    /// This method emits each transaction only once, unless we cannot guarantee the transaction's
    /// ancestors are already emitted.
    ///
    /// To understand why, consider a receiver which filters transactions based on whether it
    /// alters the UTXO set of tracked script pubkeys. If an emitted mempool transaction spends a
    /// tracked UTXO which is confirmed at height `h`, but the receiver has only seen up to block
    /// of height `h-1`, we want to re-emit this transaction until the receiver has seen the block
    /// at height `h`.
    pub fn mempool(&mut self) -> Result<Vec<(Transaction, u64)>, bitcoincore_rpc::Error> {
        let client = self.client;

        // This is the emitted tip height during the last mempool emission.
        let prev_mempool_tip = self
            .last_mempool_tip
            // We use `start_height - 1` as we cannot guarantee that the block at
            // `start_height` has been emitted.
            .unwrap_or(self.start_height.saturating_sub(1));

        // Get the raw mempool result from the RPC client which will be used to determine if any
        // transactions have been evicted.
        let raw_mempool = client.get_raw_mempool_verbose()?;
        let raw_mempool_txids: HashSet<Txid> = raw_mempool.keys().copied().collect();

        // Determine if height and hash matches the best block from the RPC. Evictions are deferred
        // if we are not at the best block.
        let height = client.get_block_count()?;
        let at_tip = if height != self.last_cp.height() as u64 {
            false
        } else {
            // Verify if block hash matches in case of re-org.
            client.get_block_hash(height)? == self.last_cp.hash()
        };

        // If at tip, any expected txid missing from raw mempool is considered evicted;
        // if not at tip, we don't evict anything.
        let mut evicted_txids: HashSet<Txid> = if at_tip {
            self.expected_mempool_txids
                .difference(&raw_mempool_txids)
                .copied()
                .collect()
        } else {
            HashSet::new()
        };

        // Mempool txs come with a timestamp of when the tx is introduced to the mempool. We keep
        // track of the latest mempool tx's timestamp to determine whether we have seen a tx
        // before. `prev_mempool_time` is the previous timestamp and `last_time` records what will
        // be the new latest timestamp.
        let prev_mempool_time = self.last_mempool_time;
        let mut latest_time = prev_mempool_time;

        let txs_to_emit = client
            .get_raw_mempool_verbose()?
            .into_iter()
            .filter_map({
                let latest_time = &mut latest_time;
                move |(txid, tx_entry)| -> Option<Result<_, bitcoincore_rpc::Error>> {
                    let tx_time = tx_entry.time as usize;
                    if tx_time > *latest_time {
                        *latest_time = tx_time;
                    }

                    // Avoid emitting transactions that are already emitted if we can guarantee
                    // blocks containing ancestors are already emitted. The bitcoind rpc interface
                    // provides us with the block height that the tx is introduced to the mempool.
                    // If we have already emitted the block of height, we can assume that all
                    // ancestor txs have been processed by the receiver.
                    let is_already_emitted = tx_time <= prev_mempool_time;
                    let is_within_height = tx_entry.height <= prev_mempool_tip as _;
                    if is_already_emitted && is_within_height {
                        return None;
                    }

                    let tx = match client.get_raw_transaction(&txid, None) {
                        Ok(tx) => tx,
                        // the tx is confirmed or evicted since `get_raw_mempool_verbose`
                        Err(err) if err.is_not_found_error() => return None,
                        Err(err) => return Some(Err(err)),
                    };

                    Some(Ok((tx, tx_time as u64)))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.last_mempool_time = latest_time;
        self.last_mempool_tip = Some(self.last_cp.height());

        // If at tip, we replace `expected_mempool_txids` with just the new txids. Otherwise, we’re
        // still catching up to the tip and keep accumulating.
        if at_tip {
            self.expected_mempool_txids = new_txs.iter().map(|(tx, _)| tx.compute_txid()).collect();
        } else {
            self.expected_mempool_txids
                .extend(new_txs.iter().map(|(tx, _)| tx.compute_txid()));
        }

        Ok(MempoolEvent {
            new_txs,
            evicted_txids,
            latest_update_time: latest_time as u64,
        })
    }

    /// Emit the next block height and header (if any).
    pub fn next_header(&mut self) -> Result<Option<BlockEvent<Header>>, bitcoincore_rpc::Error> {
        Ok(poll(self, |hash| self.client.get_block_header(hash))?
            .map(|(checkpoint, block)| BlockEvent { block, checkpoint }))
    }

    /// Emit the next block height and block (if any).
    pub fn next_block(&mut self) -> Result<Option<BlockEvent<Block>>, bitcoincore_rpc::Error> {
        if let Some((checkpoint, block)) = poll(self, |hash| self.client.get_block(hash))? {
            // Stop tracking unconfirmed transactions that have been confirmed in this block.
            for tx in &block.txdata {
                self.expected_mempool_txids.remove(&tx.compute_txid());
            }
            return Ok(Some(BlockEvent { block, checkpoint }));
        }
        Ok(None)
    }
}

/// A new emission from mempool.
#[derive(Debug)]
pub struct MempoolEvent {
    /// Unemitted transactions or transactions with ancestors that are unseen by the receiver.
    ///
    /// To understand the second condition, consider a receiver which filters transactions based on
    /// whether it alters the UTXO set of tracked script pubkeys. If an emitted mempool transaction
    /// spends a tracked UTXO which is confirmed at height `h`, but the receiver has only seen up to
    /// block of height `h-1`, we want to re-emit this transaction until the receiver has seen the
    /// block at height `h`.
    pub new_txs: Vec<(Transaction, u64)>,

    /// [`Txid`]s of all transactions that have been evicted from mempool.
    pub evicted_txids: HashSet<Txid>,

    /// The latest timestamp of when a transaction entered the mempool.
    ///
    /// This is useful for setting the timestamp for evicted transactions.
    pub latest_update_time: u64,
}

/// A newly emitted block from [`Emitter`].
#[derive(Debug)]
pub struct BlockEvent<B> {
    /// Either a full [`Block`] or [`Header`] of the new block.
    pub block: B,

    /// The checkpoint of the new block.
    ///
    /// A [`CheckPoint`] is a node of a linked list of [`BlockId`]s. This checkpoint is linked to
    /// all [`BlockId`]s originally passed in [`Emitter::new`] as well as emitted blocks since then.
    /// These blocks are guaranteed to be of the same chain.
    ///
    /// This is important as BDK structures require block-to-apply to be connected with another
    /// block in the original chain.
    pub checkpoint: CheckPoint,
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

    /// The [`BlockId`] of a previous block that this block connects to.
    ///
    /// This either returns a [`BlockId`] of a previously emitted block or from the chain we started
    /// with (passed in as `last_cp` in [`Emitter::new`]).
    ///
    /// This value is derived from [`BlockEvent::checkpoint`].
    pub fn connected_to(&self) -> BlockId {
        match self.checkpoint.prev() {
            Some(prev_cp) => prev_cp.block_id(),
            // there is no previous checkpoint, so just connect with itself
            None => self.checkpoint.block_id(),
        }
    }
}

enum PollResponse {
    Block(bitcoincore_rpc_json::GetBlockResult),
    NoMoreBlocks,
    /// Fetched block is not in the best chain.
    BlockNotInBestChain,
    AgreementFound(bitcoincore_rpc_json::GetBlockResult, CheckPoint),
    /// Force the genesis checkpoint down the receiver's throat.
    AgreementPointNotFound(BlockHash),
}

fn poll_once<C>(emitter: &Emitter<C>) -> Result<PollResponse, bitcoincore_rpc::Error>
where
    C: bitcoincore_rpc::RpcApi,
{
    let client = emitter.client;

    if let Some(last_res) = &emitter.last_block {
        let next_hash = if last_res.height < emitter.start_height as _ {
            // enforce start height
            let next_hash = client.get_block_hash(emitter.start_height as _)?;
            // make sure last emission is still in best chain
            if client.get_block_hash(last_res.height as _)? != last_res.hash {
                return Ok(PollResponse::BlockNotInBestChain);
            }
            next_hash
        } else {
            match last_res.nextblockhash {
                None => return Ok(PollResponse::NoMoreBlocks),
                Some(next_hash) => next_hash,
            }
        };

        let res = client.get_block_info(&next_hash)?;
        if res.confirmations < 0 {
            return Ok(PollResponse::BlockNotInBestChain);
        }

        return Ok(PollResponse::Block(res));
    }

    for cp in emitter.last_cp.iter() {
        let res = match client.get_block_info(&cp.hash()) {
            // block not in best chain
            Ok(res) if res.confirmations < 0 => continue,
            Ok(res) => res,
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
        return Ok(PollResponse::AgreementFound(res, cp));
    }

    let genesis_hash = client.get_block_hash(0)?;
    Ok(PollResponse::AgreementPointNotFound(genesis_hash))
}

fn poll<C, V, F>(
    emitter: &mut Emitter<C>,
    get_item: F,
) -> Result<Option<(CheckPoint, V)>, bitcoincore_rpc::Error>
where
    C: bitcoincore_rpc::RpcApi,
    F: Fn(&BlockHash) -> Result<V, bitcoincore_rpc::Error>,
{
    loop {
        match poll_once(emitter)? {
            PollResponse::Block(res) => {
                let height = res.height as u32;
                let hash = res.hash;
                let item = get_item(&hash)?;

                let new_cp = emitter
                    .last_cp
                    .clone()
                    .push(BlockId { height, hash })
                    .expect("must push");
                emitter.last_cp = new_cp.clone();
                emitter.last_block = Some(res);
                return Ok(Some((new_cp, item)));
            }
            PollResponse::NoMoreBlocks => {
                emitter.last_block = None;
                return Ok(None);
            }
            PollResponse::BlockNotInBestChain => {
                emitter.last_block = None;
                continue;
            }
            PollResponse::AgreementFound(res, cp) => {
                let agreement_h = res.height as u32;

                // The tip during the last mempool emission needs to in the best chain, we reduce
                // it if it is not.
                if let Some(h) = emitter.last_mempool_tip.as_mut() {
                    if *h > agreement_h {
                        *h = agreement_h;
                    }
                }

                // get rid of evicted blocks
                emitter.last_cp = cp;
                emitter.last_block = Some(res);
                continue;
            }
            PollResponse::AgreementPointNotFound(genesis_hash) => {
                emitter.last_cp = CheckPoint::new(BlockId {
                    height: 0,
                    hash: genesis_hash,
                });
                emitter.last_block = None;
                continue;
            }
        }
    }
}

/// Extends [`bitcoincore_rpc::Error`].
pub trait BitcoindRpcErrorExt {
    /// Returns whether the error is a "not found" error.
    ///
    /// This is useful since [`Emitter`] emits [`Result<_, bitcoincore_rpc::Error>`]s as
    /// [`Iterator::Item`].
    fn is_not_found_error(&self) -> bool;
}

impl BitcoindRpcErrorExt for bitcoincore_rpc::Error {
    fn is_not_found_error(&self) -> bool {
        if let bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(rpc_err)) = self
        {
            rpc_err.code == -5
        } else {
            false
        }
    }
}
