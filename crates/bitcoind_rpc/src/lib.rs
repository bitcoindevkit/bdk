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
//! [`Emitter::into_iterator<B>`] transforms the emitter into an iterator that returns
//! [`Emission<B>`] items. The `B` generic can either be [`EmittedBlock`] or [`EmittedHeader`] and
//! determines whether we are iterating blocks or headers.
//!
//! The iterator initially returns blocks/headers in increasing height order. After the chain tip is
//! reached, the next update is the mempool. After the mempool update is released, the first
//! succeeding call to [`Iterator::next`] will return [`None`]. Subsequent calls will resume
//! returning [`Some`] once more blocks are found.
//!
//! ```rust,no_run
//! use bdk_bitcoind_rpc::{Emission, EmittedBlock, Emitter};
//! # let client: bdk_bitcoind_rpc::bitcoincore_rpc::Client = todo!();
//!
//! for r in Emitter::new(&client, 709_632).into_iterator::<EmittedBlock>() {
//!     let emission = r.expect("todo: handle error");
//!     match emission {
//!         Emission::Block(b) => println!("block {}: {}", b.height, b.block.block_hash()),
//!         Emission::Mempool(m) => println!("mempool: {} txs", m.len()),
//!     }
//! }
//! ```
#![warn(missing_docs)]

use std::{collections::BTreeMap, marker::PhantomData};

use bitcoin::{block::Header, Block, BlockHash, Transaction};
pub use bitcoincore_rpc;
use bitcoincore_rpc::bitcoincore_rpc_json;

/// Represents a transaction that exists in the mempool.
pub struct MempoolTx {
    /// The transaction.
    pub tx: Transaction,
    /// Time when transaction first entered the mempool (in epoch seconds).
    pub time: u64,
}

/// A block obtained from `bitcoind`.
pub struct EmittedBlock {
    /// The actual block.
    pub block: Block,
    /// The height of the block.
    pub height: u32,
}

/// A block header obtained from `bitcoind`.
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

    emitted: BTreeMap<u32, BlockHash>,
    last: Option<bitcoincore_rpc_json::GetBlockResult>,
}

impl<'c, C: bitcoincore_rpc::RpcApi> Emitter<'c, C> {
    /// Constructs a new [`Emitter`] with the provided [`bitcoincore_rpc::Client`].
    ///
    /// `start_height` is the block height to start emitting blocks from.
    pub fn new(client: &'c C, start_height: u32) -> Self {
        Self {
            client,
            start_height,
            emitted: BTreeMap::new(),
            last: None,
        }
    }

    /// Emit all mempool transactions.
    pub fn mempool(&self) -> impl Iterator<Item = Result<MempoolTx, bitcoincore_rpc::Error>> + '_ {
        let res = self.client.get_raw_mempool();

        let tx_iter = res.as_ref().unwrap_or(&Vec::new()).clone().into_iter().map(
            |txid| -> Result<MempoolTx, bitcoincore_rpc::Error> {
                let tx = self.client.get_raw_transaction(&txid, None)?;
                let time = self.client.get_mempool_entry(&txid)?.time;
                Ok(MempoolTx { tx, time })
            },
        );
        // iterator will be non-empty if get_raw_mempool returned an error
        let err_iter = res.err().into_iter().map(Err);

        err_iter.chain(tx_iter)
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

    /// Transforms `self` into an iterator of [`Emission`]s.
    ///
    /// Refer to [module-level documentation] for more.
    ///
    /// [module-level documentation]: crate
    pub fn into_iterator<B>(self) -> Iter<'c, C, B> {
        Iter {
            emitter: self,
            last_emission_was_mempool: false,
            phantom: PhantomData,
        }
    }
}

/// This is the [`Iterator::Item`] of [`Iter`]. This can either represent a block/header or the set
/// of mempool transactions.
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub enum Emission<B> {
    /// An emitted set of mempool transactions.
    Mempool(Vec<MempoolTx>),
    /// An emitted block.
    Block(B),
}

impl<B> Emission<B> {
    /// Whether the emission if of mempool transactions.
    pub fn is_mempool(&self) -> bool {
        matches!(self, Self::Mempool(_))
    }

    /// Wether the emission if of a block.
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block(_))
    }
}

/// An [`Iterator`] that wraps an [`Emitter`], and emits [`Result`]s of [`Emission`].
///
/// This is constructed with [`Emitter::into_iterator`].
pub struct Iter<'c, C, B = EmittedBlock> {
    emitter: Emitter<'c, C>,
    last_emission_was_mempool: bool,
    phantom: PhantomData<B>,
}

impl<'c, C: bitcoincore_rpc::RpcApi, B> Iter<'c, C, B> {
    fn next_with<F>(&mut self, f: F) -> Option<Result<Emission<B>, bitcoincore_rpc::Error>>
    where
        F: Fn(&mut Emitter<'c, C>) -> Result<Option<B>, bitcoincore_rpc::Error>,
    {
        if self.last_emission_was_mempool {
            self.last_emission_was_mempool = false;
            return None;
        }

        match f(&mut self.emitter) {
            Ok(None) => {
                self.last_emission_was_mempool = true;
                Some(
                    self.emitter
                        .mempool()
                        .collect::<Result<Vec<_>, _>>()
                        .map(Emission::<B>::Mempool),
                )
            }
            Ok(Some(b)) => Some(Ok(Emission::Block(b))),
            Err(err) => Some(Err(err)),
        }
    }
}

impl<'c, C: bitcoincore_rpc::RpcApi> Iterator for Iter<'c, C, EmittedBlock> {
    type Item = Result<Emission<EmittedBlock>, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_with(Emitter::next_block)
    }
}

impl<'c, C: bitcoincore_rpc::RpcApi> Iterator for Iter<'c, C, EmittedHeader> {
    type Item = Result<Emission<EmittedHeader>, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_with(Emitter::next_header)
    }
}

enum PollResponse {
    Block(bitcoincore_rpc_json::GetBlockResult),
    NoMoreBlocks,
    BlockNotInBestChain,
    AgreementFound(bitcoincore_rpc_json::GetBlockResult),
    AgreementPointNotFound,
}

fn poll_once<C>(emitter: &Emitter<C>) -> Result<PollResponse, bitcoincore_rpc::Error>
where
    C: bitcoincore_rpc::RpcApi,
{
    let client = emitter.client;

    if let Some(last_res) = &emitter.last {
        assert!(!emitter.emitted.is_empty());

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

    if emitter.emitted.is_empty() {
        let hash = client.get_block_hash(emitter.start_height as _)?;

        let res = client.get_block_info(&hash)?;
        if res.confirmations < 0 {
            return Ok(PollResponse::BlockNotInBestChain);
        }
        return Ok(PollResponse::Block(res));
    }

    for (&_, hash) in emitter.emitted.iter().rev() {
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
                assert_eq!(emitter.emitted.insert(height, res.hash), None);
                emitter.last = Some(res);
                return Ok(Some((height, item)));
            }
            PollResponse::NoMoreBlocks => {
                emitter.last = None;
                return Ok(None);
            }
            PollResponse::BlockNotInBestChain => {
                emitter.last = None;
                continue;
            }
            PollResponse::AgreementFound(res) => {
                emitter.emitted.split_off(&(res.height as u32 + 1));
                emitter.last = Some(res);
                continue;
            }
            PollResponse::AgreementPointNotFound => {
                emitter.emitted.clear();
                emitter.last = None;
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
