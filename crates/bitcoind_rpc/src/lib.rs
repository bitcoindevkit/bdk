//! This crate is used for emitting blockchain data from the `bitcoind` RPC interface (excluding the
//! RPC wallet API).
//!
//! [`Emitter`] is the main structure which sources blockchain data from [`bitcoincore_rpc::Client`].
//!
//! To only get block updates (exlude mempool transactions), the caller can use
//! [`Emitter::next_block`] or/and [`Emitter::next_header`] until it returns `Ok(None)` (which means
//! the chain tip is reached). A separate method, [`Emitter::mempool`] can be used to emit the whole
//! mempool.
//!
//! # [`Iter`]
//!
//! [`Emitter::into_iterator<B>`] transforms the emitter into an iterator that either returns
//! [`EmittedBlock`] or [`EmittedHeader`].
//!
//! ```rust,no_run
//! use bdk_bitcoind_rpc::{EmittedBlock, Emitter};
//! # let client: bdk_bitcoind_rpc::bitcoincore_rpc::Client = todo!();
//!
//! for r in Emitter::new(&client, 709_632).into_iterator::<EmittedBlock>() {
//!     let emitted_block = r.expect("todo: handle error");
//!     println!(
//!         "block {}: {}",
//!         emitted_block.height,
//!         emitted_block.block.block_hash()
//!     );
//! }
//! ```
#![warn(missing_docs)]

use std::{collections::BTreeMap, marker::PhantomData};

use bitcoin::{block::Header, Block, BlockHash, Transaction};
pub use bitcoincore_rpc;
use bitcoincore_rpc::bitcoincore_rpc_json;

/// Represents a transaction that exists in the mempool.
#[derive(Debug)]
pub struct MempoolTx {
    /// The transaction.
    pub tx: Transaction,
    /// Time when transaction first entered the mempool (in epoch seconds).
    pub time: u64,
}

/// A block obtained from `bitcoind`.
#[derive(Debug)]
pub struct EmittedBlock {
    /// The actual block.
    pub block: Block,
    /// The height of the block.
    pub height: u32,
}

/// A block header obtained from `bitcoind`.
#[derive(Debug)]
pub struct EmittedHeader {
    /// The actual block header.
    pub header: Header,
    /// The height of the block header.
    pub height: u32,
}

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
    last_mempool_tip: Option<(u32, BlockHash)>,
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

    /// Emit mempool transactions.
    ///
    /// This avoids re-emitting transactions (where viable). We can do this if all blocks
    /// containing ancestor transactions are already emitted.
    pub fn mempool(&mut self) -> Result<Vec<MempoolTx>, bitcoincore_rpc::Error> {
        let client = self.client;

        let prev_mempool_tip = match self.last_mempool_tip {
            // use 'avoid-re-emission' logic if there is no reorg
            Some((height, hash)) if self.emitted_blocks.get(&height) == Some(&hash) => height,
            _ => 0,
        };

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
                        Err(err) => return Some(Err(err)),
                    };

                    Some(Ok(MempoolTx {
                        tx,
                        time: tx_time as u64,
                    }))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.last_mempool_time = latest_time;
        self.last_mempool_tip = self
            .emitted_blocks
            .iter()
            .last()
            .map(|(&height, &hash)| (height, hash));

        Ok(txs_to_emit)
    }

    /// Emit the next block header (if any).
    pub fn next_header(&mut self) -> Result<Option<EmittedHeader>, bitcoincore_rpc::Error> {
        let poll_res = poll(self, |hash| self.client.get_block_header(hash))?;
        Ok(poll_res.map(|(height, header)| EmittedHeader { header, height }))
    }

    /// Emit the next block (if any).
    pub fn next_block(&mut self) -> Result<Option<EmittedBlock>, bitcoincore_rpc::Error> {
        let poll_res = poll(self, |hash| self.client.get_block(hash))?;
        Ok(poll_res.map(|(height, block)| EmittedBlock { block, height }))
    }

    /// Transforms `self` into an iterator of either [`EmittedBlock`]s or [`EmittedHeader`]s.
    ///
    /// Refer to [module-level documentation] for more.
    ///
    /// [module-level documentation]: crate
    pub fn into_iterator<B>(self) -> Iter<'c, C, B> {
        Iter {
            emitter: self,
            phantom: PhantomData,
        }
    }
}

/// An [`Iterator`] that wraps an [`Emitter`], and emits [`Result`]s of either [`EmittedHeader`]s
/// or [`EmittedBlock`]s.
///
/// This is constructed with [`Emitter::into_iterator`].
pub struct Iter<'c, C, B = EmittedBlock> {
    emitter: Emitter<'c, C>,
    phantom: PhantomData<B>,
}

impl<'c, C: bitcoincore_rpc::RpcApi> Iterator for Iter<'c, C, EmittedBlock> {
    type Item = Result<EmittedBlock, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.emitter.next_block().transpose()
    }
}

impl<'c, C: bitcoincore_rpc::RpcApi> Iterator for Iter<'c, C, EmittedHeader> {
    type Item = Result<EmittedHeader, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.emitter.next_header().transpose()
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
                emitter.emitted_blocks.split_off(&(res.height as u32 + 1));
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
