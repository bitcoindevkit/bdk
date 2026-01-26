//! This crate is used for emitting blockchain data from the `bitcoind` RPC interface. It does not
//! use the wallet RPC API, so this crate can be used with wallet-disabled Bitcoin Core nodes.
//!
//! [`Emitter`] is the main structure which sources blockchain data from
//! [`bitcoincore_rpc::Client`].
//!
//! To only get block updates (exclude mempool transactions), the caller can use
//! [`Emitter::next_block`] until it returns `Ok(None)` (which means the chain tip is reached). A
//! separate method, [`Emitter::mempool`] can be used to emit the whole mempool.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

use alloc::sync::Arc;
use bdk_core::collections::{HashMap, HashSet};
use bdk_core::{BlockId, CheckPoint};
use bitcoin::{Block, BlockHash, Transaction, Txid};
use bitcoincore_rpc::{bitcoincore_rpc_json, RpcApi};
use core::ops::Deref;

pub mod bip158;

pub use bitcoincore_rpc;

/// The [`Emitter`] is used to emit data sourced from [`bitcoincore_rpc::Client`].
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub struct Emitter<C> {
    client: C,
    start_height: u32,

    /// The checkpoint of the last-emitted block that is in the best chain. If it is later found
    /// that the block is no longer in the best chain, it will be popped off from here.
    last_cp: CheckPoint<BlockHash>,

    /// The block result returned from rpc of the last-emitted block. As this result contains the
    /// next block's block hash (which we use to fetch the next block), we set this to `None`
    /// whenever there are no more blocks, or the next block is no longer in the best chain. This
    /// gives us an opportunity to re-fetch this result.
    last_block: Option<bitcoincore_rpc_json::GetBlockResult>,

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

/// Indicates that there are no initially-expected mempool transactions.
///
/// Use this as the `expected_mempool_txs` field of [`Emitter::new`] when the wallet is known
/// to start empty (i.e. with no unconfirmed transactions).
pub const NO_EXPECTED_MEMPOOL_TXS: core::iter::Empty<Arc<Transaction>> = core::iter::empty();

impl<C> Emitter<C>
where
    C: Deref,
    C::Target: RpcApi,
{
    /// Construct a new [`Emitter`].
    ///
    /// `last_cp` informs the emitter of the chain we are starting off with. This way, the emitter
    /// can start emission from a block that connects to the original chain.
    ///
    /// `start_height` starts emission from a given height (if there are no conflicts with the
    /// original chain).
    ///
    /// `expected_mempool_txs` is the initial set of unconfirmed transactions provided by the
    /// wallet. This allows the [`Emitter`] to inform the wallet about relevant mempool evictions.
    /// If it is known that the wallet is empty, [`NO_EXPECTED_MEMPOOL_TXS`] can be used.
    pub fn new(
        client: C,
        last_cp: CheckPoint<BlockHash>,
        start_height: u32,
        expected_mempool_txs: impl IntoIterator<Item = impl Into<Arc<Transaction>>>,
    ) -> Self {
        Self {
            client,
            start_height,
            last_cp,
            last_block: None,
            mempool_snapshot: expected_mempool_txs
                .into_iter()
                .map(|tx| {
                    let tx: Arc<Transaction> = tx.into();
                    (tx.compute_txid(), tx)
                })
                .collect(),
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
    pub fn mempool(&mut self) -> Result<MempoolEvent, bitcoincore_rpc::Error> {
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
    pub fn mempool_at(&mut self, sync_time: u64) -> Result<MempoolEvent, bitcoincore_rpc::Error> {
        let client = &*self.client;

        let mut rpc_tip_height;
        let mut rpc_tip_hash;
        let mut rpc_mempool;
        let mut rpc_mempool_txids;

        // Ensure we get a mempool snapshot consistent with `rpc_tip_hash` as the tip.
        loop {
            rpc_tip_height = client.get_block_count()?;
            rpc_tip_hash = client.get_block_hash(rpc_tip_height)?;
            rpc_mempool = client.get_raw_mempool()?;
            rpc_mempool_txids = rpc_mempool.iter().copied().collect::<HashSet<Txid>>();
            let is_still_at_tip = rpc_tip_hash == client.get_block_hash(rpc_tip_height)?
                && rpc_tip_height == client.get_block_count()?;
            if is_still_at_tip {
                break;
            }
        }

        let mut mempool_event = MempoolEvent {
            update: rpc_mempool
                .into_iter()
                .filter_map(|txid| -> Option<Result<_, bitcoincore_rpc::Error>> {
                    let tx = match self.mempool_snapshot.get(&txid) {
                        Some(tx) => tx.clone(),
                        None => match client.get_raw_transaction(&txid, None) {
                            Ok(tx) => {
                                let tx = Arc::new(tx);
                                self.mempool_snapshot.insert(txid, tx.clone());
                                tx
                            }
                            Err(err) if err.is_not_found_error() => return None,
                            Err(err) => return Some(Err(err)),
                        },
                    };
                    Some(Ok((tx, sync_time)))
                })
                .collect::<Result<Vec<_>, _>>()?,
            ..Default::default()
        };

        let at_tip =
            rpc_tip_height == self.last_cp.height() as u64 && rpc_tip_hash == self.last_cp.hash();

        if at_tip {
            // We only emit evicted transactions when we have already emitted the RPC tip. This is
            // because we cannot differentiate between transactions that are confirmed and
            // transactions that are evicted, so we rely on emitted blocks to remove
            // transactions from the `mempool_snapshot`.
            mempool_event.evicted = self
                .mempool_snapshot
                .keys()
                .filter(|&txid| !rpc_mempool_txids.contains(txid))
                .map(|&txid| (txid, sync_time))
                .collect();
            self.mempool_snapshot = mempool_event
                .update
                .iter()
                .map(|(tx, _)| (tx.compute_txid(), tx.clone()))
                .collect();
        } else {
            // Since we are still catching up to the tip (a.k.a tip has not been emitted), we
            // accumulate more transactions in `mempool_snapshot` so that we can emit evictions in
            // a batch once we catch up.
            self.mempool_snapshot.extend(
                mempool_event
                    .update
                    .iter()
                    .map(|(tx, _)| (tx.compute_txid(), tx.clone())),
            );
        };

        Ok(mempool_event)
    }

    /// Emit the next block height and block (if any).
    pub fn next_block(&mut self) -> Result<Option<BlockEvent<Block>>, bitcoincore_rpc::Error> {
        if let Some((checkpoint, block)) = poll(self, move |hash, client| client.get_block(hash))? {
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
pub struct BlockEvent<B> {
    /// The block.
    pub block: B,

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
    AgreementFound(bitcoincore_rpc_json::GetBlockResult, CheckPoint<BlockHash>),
    /// Force the genesis checkpoint down the receiver's throat.
    AgreementPointNotFound(BlockHash),
}

fn poll_once<C>(emitter: &Emitter<C>) -> Result<PollResponse, bitcoincore_rpc::Error>
where
    C: Deref,
    C::Target: RpcApi,
{
    let client = &*emitter.client;

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
) -> Result<Option<(CheckPoint<BlockHash>, V)>, bitcoincore_rpc::Error>
where
    C: Deref,
    C::Target: RpcApi,
    F: Fn(&BlockHash, &C::Target) -> Result<V, bitcoincore_rpc::Error>,
{
    loop {
        match poll_once(emitter)? {
            PollResponse::Block(res) => {
                let height = res.height as u32;
                let hash = res.hash;
                let item = get_item(&hash, &emitter.client)?;

                let new_cp = emitter
                    .last_cp
                    .clone()
                    .push(height, hash)
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
                // get rid of evicted blocks
                emitter.last_cp = cp;
                emitter.last_block = Some(res);
                continue;
            }
            PollResponse::AgreementPointNotFound(genesis_hash) => {
                emitter.last_cp = CheckPoint::new(0, genesis_hash);
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use crate::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
    use bdk_chain::local_chain::LocalChain;
    use bdk_testenv::{anyhow, TestEnv};
    use bitcoin::{hashes::Hash, Address, Amount, ScriptBuf, Txid, WScriptHash};
    use std::collections::HashSet;

    #[test]
    fn test_expected_mempool_txids_accumulate_and_remove() -> anyhow::Result<()> {
        let env = TestEnv::new()?;
        let (chain, _) = LocalChain::from_genesis(env.genesis_hash()?);
        let chain_tip = chain.tip();

        let rpc_client = bitcoincore_rpc::Client::new(
            &env.bitcoind.rpc_url(),
            bitcoincore_rpc::Auth::CookieFile(env.bitcoind.params.cookie_file.clone()),
        )?;

        let mut emitter = Emitter::new(&rpc_client, chain_tip.clone(), 1, NO_EXPECTED_MEMPOOL_TXS);

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
