//! This crate is used for emitting blockchain data from the `bitcoind` RPC interface (excluding the
//! RPC wallet API).
//!
//! [`Emitter`] is the main structure which sources blockchain data from [`bitcoincore_rpc::Client`].
//!
//! To only get block updates (exclude mempool transactions), the caller can use
//! [`Emitter::next_block`] or/and [`Emitter::next_header`] until it returns `Ok(None)` (which means
//! the chain tip is reached). A separate method, [`Emitter::mempool`] can be used to emit the whole
//! mempool.
#![warn(missing_docs)]

use std::collections::BTreeMap;

use bitcoin::{block::Header, Block, BlockHash, Transaction};
pub use bitcoincore_rpc;
use bitcoincore_rpc::bitcoincore_rpc_json;

/// A structure that emits data sourced from [`bitcoincore_rpc::Client`].
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub struct Emitter<'c, C> {
    client: &'c C,
    start_height: u32,

    emitted_blocks: BTreeMap<u32, BlockHash>,
    last_block: Option<bitcoincore_rpc_json::GetBlockResult>,

    /// The latest first-seen epoch of emitted mempool transactions. This is used to determine
    /// whether a mempool transaction is already emitted.
    last_mempool_time: usize,

    /// The last emitted block during our last mempool emission. This is used to determine whether
    /// there has been a reorg since our last mempool emission.
    last_mempool_tip: Option<u32>,
}

impl<'c, C: bitcoincore_rpc::RpcApi> Emitter<'c, C> {
    /// Constructs a new [`Emitter`] with the provided [`bitcoincore_rpc::Client`].
    ///
    /// `start_height` is the block height to start emitting blocks from.
    pub fn new(client: &'c C, start_height: u32) -> Self {
        Self {
            client,
            start_height,
            emitted_blocks: BTreeMap::new(),
            last_block: None,
            last_mempool_time: 0,
            last_mempool_tip: None,
        }
    }

    /// Emit mempool transactions, alongside their first-seen unix timestamps.
    ///
    /// Ideally, this method would only emit the same transaction once. However, if the receiver
    /// filters transactions based on whether it alters the output set of tracked script pubkeys,
    /// there are situations where we would want to re-emit. For example, if an emitted mempool
    /// transaction spends a tracked UTXO which is confirmed at height `h`, but the receiver has
    /// only seen up to block of height `h-1`, we want to re-emit this transaction until the
    /// receiver has seen the block at height `h`.
    ///
    /// In other words, we want to re-emit a transaction if we cannot guarantee it's ancestors are
    /// already emitted.
    pub fn mempool(&mut self) -> Result<Vec<(Transaction, u64)>, bitcoincore_rpc::Error> {
        let client = self.client;

        // This is the emitted tip height during the last mempool emission.
        let prev_mempool_tip = self
            .last_mempool_tip
            // We use `start_height - 1` as we cannot guarantee that the block at
            // `start_height` has been emitted.
            .unwrap_or(self.start_height.saturating_sub(1));

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
        self.last_mempool_tip = self.emitted_blocks.iter().last().map(|(&height, _)| height);

        Ok(txs_to_emit)
    }

    /// Emit the next block height and header (if any).
    pub fn next_header(&mut self) -> Result<Option<(u32, Header)>, bitcoincore_rpc::Error> {
        poll(self, |hash| self.client.get_block_header(hash))
    }

    /// Emit the next block height and block (if any).
    pub fn next_block(&mut self) -> Result<Option<(u32, Block)>, bitcoincore_rpc::Error> {
        poll(self, |hash| self.client.get_block(hash))
    }
}

enum PollResponse {
    Block(bitcoincore_rpc_json::GetBlockResult),
    NoMoreBlocks,
    /// Fetched block is not in the best chain.
    BlockNotInBestChain,
    AgreementFound(bitcoincore_rpc_json::GetBlockResult),
    AgreementPointNotFound,
}

fn poll_once<C>(emitter: &Emitter<C>) -> Result<PollResponse, bitcoincore_rpc::Error>
where
    C: bitcoincore_rpc::RpcApi,
{
    let client = emitter.client;

    if let Some(last_res) = &emitter.last_block {
        assert!(!emitter.emitted_blocks.is_empty());

        let next_hash = match last_res.nextblockhash {
            None => return Ok(PollResponse::NoMoreBlocks),
            Some(next_hash) => next_hash,
        };

        let res = client.get_block_info(&next_hash)?;
        if res.confirmations < 0 {
            return Ok(PollResponse::BlockNotInBestChain);
        }
        return Ok(PollResponse::Block(res));
    }

    if emitter.emitted_blocks.is_empty() {
        let hash = client.get_block_hash(emitter.start_height as _)?;

        let res = client.get_block_info(&hash)?;
        if res.confirmations < 0 {
            return Ok(PollResponse::BlockNotInBestChain);
        }
        return Ok(PollResponse::Block(res));
    }

    for (&_, hash) in emitter.emitted_blocks.iter().rev() {
        let res = client.get_block_info(hash)?;
        if res.confirmations < 0 {
            // block is not in best chain
            continue;
        }

        // agreement point found
        return Ok(PollResponse::AgreementFound(res));
    }

    Ok(PollResponse::AgreementPointNotFound)
}

fn poll<C, V, F>(
    emitter: &mut Emitter<C>,
    get_item: F,
) -> Result<Option<(u32, V)>, bitcoincore_rpc::Error>
where
    C: bitcoincore_rpc::RpcApi,
    F: Fn(&BlockHash) -> Result<V, bitcoincore_rpc::Error>,
{
    loop {
        match poll_once(emitter)? {
            PollResponse::Block(res) => {
                let height = res.height as u32;
                let item = get_item(&res.hash)?;
                assert_eq!(emitter.emitted_blocks.insert(height, res.hash), None);
                emitter.last_block = Some(res);
                return Ok(Some((height, item)));
            }
            PollResponse::NoMoreBlocks => {
                emitter.last_block = None;
                return Ok(None);
            }
            PollResponse::BlockNotInBestChain => {
                emitter.last_block = None;
                continue;
            }
            PollResponse::AgreementFound(res) => {
                let agreement_h = res.height as u32;

                // get rid of evicted blocks
                emitter.emitted_blocks.split_off(&(agreement_h + 1));

                // The tip during the last mempool emission needs to in the best chain, we reduce
                // it if it is not.
                if let Some(h) = emitter.last_mempool_tip.as_mut() {
                    if *h > agreement_h {
                        *h = agreement_h;
                    }
                }
                emitter.last_block = Some(res);
                continue;
            }
            PollResponse::AgreementPointNotFound => {
                emitter.emitted_blocks.clear();
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
