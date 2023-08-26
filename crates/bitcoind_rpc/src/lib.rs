//! This crate is used for updating [`bdk_chain`] structures with data from the `bitcoind` RPC
//! interface (excluding the RPC wallet API).
//!
//! [`Emitter`] is the main structure which sources blockchain data from [`bitcoincore_rpc::Client`].
//!
//! To only get block updates (exlude mempool transactions), the caller can use
//! [`Emitter::emit_block`] until it returns `Ok(None)` (which means the chain tip is reached). A
//! separate method, [`Emitter::emit_mempool`] can be used to emit the whole mempool. Another
//! method, [`Emitter::emit_update`] is avaliable, which emits block updates until the block tip is
//! reached, then the next update will be the mempool.
//!
//! # [`IntoIterator`] implementation
//!
//! [`Emitter`] implements [`IntoIterator`] which transforms itself into [`UpdateIter`]. The
//! iterator is implemented in a way that even after a call to [`Iterator::next`] returns [`None`],
//! subsequent calls may resume returning [`Some`].
//!
//! The iterator initially returns blocks in increasing height order. After the chain tip is
//! reached, the next update is the mempool. After the mempool update is released, the first
//! succeeding call to [`Iterator::next`] will return [`None`].
//!
//! This logic is useful if the caller wishes to "update once".
//!
//! ```rust,no_run
//! use bdk_bitcoind_rpc::{EmittedUpdate, Emitter};
//! # let client: bdk_bitcoind_rpc::bitcoincore_rpc::Client = todo!();
//!
//! for r in Emitter::new(&client, 709_632, None) {
//!     let update = r.expect("todo: deal with the error properly");
//!
//!     match update.checkpoint() {
//!         Some(cp) => println!("block {}:{}", cp.height(), cp.hash()),
//!         None => println!("mempool!"),
//!     }
//! }
//! ```
//!
//! Alternatively, if the caller wishes to keep [`Emitter`] in a dedicated update-thread, the caller
//! can continue to poll [`Iterator::next`] with a delay.

#![warn(missing_docs)]

use bdk_chain::{
    bitcoin::{Block, Transaction},
    indexed_tx_graph::TxItem,
    local_chain::{self, CheckPoint},
    BlockId, ConfirmationHeightAnchor, ConfirmationTimeAnchor,
};
pub use bitcoincore_rpc;
use bitcoincore_rpc::{json::GetBlockResult, RpcApi};
use std::fmt::Debug;

/// An update emitted from [`Emitter`]. This can either be of a block or a subset of
/// mempool transactions.
#[derive(Debug, Clone)]
pub enum EmittedUpdate {
    /// An emitted block.
    Block(EmittedBlock),
    /// An emitted subset of mempool transactions.
    ///
    /// [`Emitter`] attempts to avoid re-emitting transactions.
    Mempool(EmittedMempool),
}

impl EmittedUpdate {
    /// Returns whether the update is of a subset of the mempool.
    pub fn is_mempool(&self) -> bool {
        matches!(self, Self::Mempool { .. })
    }

    /// Returns whether the update is of a block.
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    /// Get the emission's checkpoint.
    ///
    /// The emission will only have a checkpoint if it is the [`EmittedUpdate::Block`] variant.
    pub fn checkpoint(&self) -> Option<CheckPoint> {
        match self {
            EmittedUpdate::Block(e) => Some(e.checkpoint()),
            EmittedUpdate::Mempool(_) => None,
        }
    }

    /// Convenience method to get [`local_chain::Update`].
    pub fn chain_update(&self) -> Option<local_chain::Update> {
        Some(local_chain::Update {
            tip: self.checkpoint()?,
            introduce_older_blocks: false,
        })
    }

    /// Return transaction items to be consumed by [`IndexedTxGraph::insert_relevant_txs`].
    ///
    /// The `anchor_map` parameter takes in a closure that creates anchors of a specific type.
    /// [`confirmation_height_anchor`] and [`confirmation_time_anchor`] are avaliable to create
    /// updates with [`ConfirmationHeightAnchor`] and [`ConfirmationTimeAnchor`] respectively.
    ///
    /// [`IndexedTxGraph::insert_relevant_txs`]: bdk_chain::IndexedTxGraph::insert_relevant_txs
    pub fn indexed_tx_graph_update<M, A>(&self, anchor_map: M) -> Vec<TxItem<'_, Option<A>>>
    where
        M: Fn(&CheckPoint, &Block, usize) -> A,
        A: Clone + Ord + PartialEq,
    {
        match self {
            EmittedUpdate::Block(e) => e.indexed_tx_graph_update(anchor_map).collect(),
            EmittedUpdate::Mempool(e) => e.indexed_tx_graph_update().collect(),
        }
    }
}

/// An emitted block.
#[derive(Debug, Clone)]
pub struct EmittedBlock {
    /// The checkpoint constructed from the block's height/hash and connected to the previous block.
    pub cp: CheckPoint,
    /// The actual block of the chain.
    pub block: Block,
}

impl EmittedBlock {
    /// Get the emission's checkpoint.
    pub fn checkpoint(&self) -> CheckPoint {
        self.cp.clone()
    }

    /// Convenience method to get [`local_chain::Update`].
    pub fn chain_update(&self) -> local_chain::Update {
        local_chain::Update {
            tip: self.cp.clone(),
            introduce_older_blocks: false,
        }
    }

    /// Return transaction items to be consumed by [`IndexedTxGraph::insert_relevant_txs`].
    ///
    /// Refer to [`EmittedUpdate::indexed_tx_graph_update`] for more.
    ///
    /// [`IndexedTxGraph::insert_relevant_txs`]: bdk_chain::IndexedTxGraph::insert_relevant_txs
    pub fn indexed_tx_graph_update<M, A>(
        &self,
        anchor_map: M,
    ) -> impl Iterator<Item = TxItem<'_, Option<A>>>
    where
        M: Fn(&CheckPoint, &Block, usize) -> A,
        A: Clone + Ord + PartialEq,
    {
        self.block
            .txdata
            .iter()
            .enumerate()
            .map(move |(i, tx)| (tx, Some(anchor_map(&self.cp, &self.block, i)), None))
    }
}

/// An emitted subset of mempool transactions.
#[derive(Debug, Clone)]
pub struct EmittedMempool {
    /// Subset of mempool transactions as tuples of `(tx, seen_at)`.
    ///
    /// `seen_at` is the unix timestamp of when the transaction was first seen in the mempool.
    pub txs: Vec<(Transaction, u64)>,
}

impl EmittedMempool {
    /// Return transaction items to be consumed by [`IndexedTxGraph::insert_relevant_txs`].
    ///
    /// Refer to [`EmittedUpdate::indexed_tx_graph_update`] for more.
    ///
    /// [`IndexedTxGraph::insert_relevant_txs`]: bdk_chain::IndexedTxGraph::insert_relevant_txs
    pub fn indexed_tx_graph_update<A>(&self) -> impl Iterator<Item = TxItem<'_, Option<A>>>
    where
        A: Clone + Ord + PartialEq,
    {
        self.txs
            .iter()
            .map(|(tx, seen_at)| (tx, None, Some(*seen_at)))
    }
}

/// A closure that transforms a [`EmittedUpdate`] into a [`ConfirmationHeightAnchor`].
///
/// This is to be used as an input to [`EmittedUpdate::indexed_tx_graph_update`].
pub fn confirmation_height_anchor(
    cp: &CheckPoint,
    _block: &Block,
    _tx_pos: usize,
) -> ConfirmationHeightAnchor {
    let anchor_block = cp.block_id();
    ConfirmationHeightAnchor {
        anchor_block,
        confirmation_height: anchor_block.height,
    }
}

/// A closure that transforms a [`EmittedUpdate`] into a [`ConfirmationTimeAnchor`].
///
/// This is to be used as an input to [`EmittedUpdate::indexed_tx_graph_update`].
pub fn confirmation_time_anchor(
    cp: &CheckPoint,
    block: &Block,
    _tx_pos: usize,
) -> ConfirmationTimeAnchor {
    let anchor_block = cp.block_id();
    ConfirmationTimeAnchor {
        anchor_block,
        confirmation_height: anchor_block.height,
        confirmation_time: block.header.time as _,
    }
}

/// A structure that emits updates for [`bdk_chain`] structures, sourcing blockchain data from
/// [`bitcoincore_rpc::Client`].
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub struct Emitter<'c, C> {
    client: &'c C,
    fallback_height: u32,

    last_cp: Option<CheckPoint>,
    last_info: Option<GetBlockResult>,
}

impl<'c, C: RpcApi> IntoIterator for Emitter<'c, C> {
    type Item = <UpdateIter<'c, C> as Iterator>::Item;
    type IntoIter = UpdateIter<'c, C>;

    fn into_iter(self) -> Self::IntoIter {
        UpdateIter {
            emitter: self,
            last_emission_was_mempool: false,
        }
    }
}

impl<'c, C: RpcApi> Emitter<'c, C> {
    /// Constructs a new [`Emitter`] with the provided [`bitcoincore_rpc::Client`].
    ///
    /// * `fallback_height` is the block height to start from if `last_cp` is not provided, or a
    ///     point of agreement is not found.
    /// * `last_cp` is the last known checkpoint to build updates on (if any).
    pub fn new(client: &'c C, fallback_height: u32, last_cp: Option<CheckPoint>) -> Self {
        Self {
            client,
            fallback_height,
            last_cp,
            last_info: None,
        }
    }

    /// Emits the whole mempool contents.
    pub fn emit_mempool(&self) -> Result<EmittedMempool, bitcoincore_rpc::Error> {
        let txs = self
            .client
            .get_raw_mempool()?
            .into_iter()
            .map(
                |txid| -> Result<(Transaction, u64), bitcoincore_rpc::Error> {
                    let first_seen = self
                        .client
                        .get_mempool_entry(&txid)
                        .map(|entry| entry.time)?;
                    let tx = self.client.get_raw_transaction(&txid, None)?;
                    Ok((tx, first_seen))
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EmittedMempool { txs })
    }

    /// Emits the next block (if any).
    pub fn emit_block(&mut self) -> Result<Option<EmittedBlock>, bitcoincore_rpc::Error> {
        enum PollResponse {
            /// A new block that is in chain is found. Congratulations!
            Block {
                cp: CheckPoint,
                info: GetBlockResult,
            },
            /// This either signals that we have reached the tip, or that the blocks ahead are not
            /// in the best chain. In either case, we need to find the agreement point again.
            NoMoreBlocks,
            /// We have exhausted the local checkpoint history and there is no agreement point. We
            /// should emit from the fallback height for the next round.
            AgreementPointNotFound,
            /// We have found an agreement point! Do not emit this one, emit the one higher.
            AgreementPointFound {
                cp: CheckPoint,
                info: GetBlockResult,
            },
        }

        fn poll<C>(emitter: &mut Emitter<C>) -> Result<PollResponse, bitcoincore_rpc::Error>
        where
            C: RpcApi,
        {
            let client = emitter.client;

            match (&mut emitter.last_cp, &mut emitter.last_info) {
                (None, None) => {
                    let info = client
                        .get_block_info(&client.get_block_hash(emitter.fallback_height as _)?)?;
                    let cp = CheckPoint::new(BlockId {
                        height: info.height as _,
                        hash: info.hash,
                    });
                    Ok(PollResponse::Block { cp, info })
                }
                (Some(last_cp), None) => {
                    for cp in last_cp.iter() {
                        let cp_block = cp.block_id();
                        let info = client.get_block_info(&cp_block.hash)?;
                        if info.confirmations < 0 {
                            // block is not in the main chain
                            continue;
                        }
                        // agreement point found
                        return Ok(PollResponse::AgreementPointFound { cp, info });
                    }
                    // no agreement point found
                    Ok(PollResponse::AgreementPointNotFound)
                }
                (Some(last_cp), Some(last_info)) => {
                    let next_hash = match last_info.nextblockhash {
                        None => return Ok(PollResponse::NoMoreBlocks),
                        Some(next_hash) => next_hash,
                    };
                    let info = client.get_block_info(&next_hash)?;
                    if info.confirmations < 0 {
                        return Ok(PollResponse::NoMoreBlocks);
                    }
                    let cp = last_cp
                        .clone()
                        .push(BlockId {
                            height: info.height as _,
                            hash: info.hash,
                        })
                        .expect("must extend from checkpoint");
                    Ok(PollResponse::Block { cp, info })
                }
                (None, Some(last_info)) => unreachable!(
                    "info cannot exist without checkpoint: info={:#?}",
                    last_info
                ),
            }
        }

        loop {
            match poll(self)? {
                PollResponse::Block { cp, info } => {
                    let block = self.client.get_block(&info.hash)?;
                    self.last_cp = Some(cp.clone());
                    self.last_info = Some(info);
                    return Ok(Some(EmittedBlock { cp, block }));
                }
                PollResponse::NoMoreBlocks => {
                    // we have reached the tip, try find agreement point in next round
                    self.last_info = None;
                    return Ok(None);
                }
                PollResponse::AgreementPointNotFound => {
                    self.last_cp = None;
                    self.last_info = None;
                    continue;
                }
                PollResponse::AgreementPointFound { cp, info } => {
                    self.last_cp = Some(cp);
                    self.last_info = Some(info);
                    continue;
                }
            }
        }
    }

    /// Continuously poll [`bitcoincore_rpc::Client`] until an update is found.
    pub fn emit_update(&mut self) -> Result<EmittedUpdate, bitcoincore_rpc::Error> {
        match self.emit_block()? {
            Some(emitted_block) => Ok(EmittedUpdate::Block(emitted_block)),
            None => self.emit_mempool().map(EmittedUpdate::Mempool),
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

/// An [`Iterator`] that wraps an [`Emitter`], and emits [`Result`]s of [`EmittedUpdate`].
///
/// ```rust,no_run
/// use bdk_bitcoind_rpc::{EmittedUpdate, Emitter, UpdateIter};
/// use core::iter::{IntoIterator, Iterator};
/// # let client: bdk_bitcoind_rpc::bitcoincore_rpc::Client = todo!();
///
/// let mut update_iter = Emitter::new(&client, 706_932, None).into_iter();
/// let update = update_iter.next().expect("must get next update");
/// println!("got update: {:?}", update);
/// ```
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate
pub struct UpdateIter<'c, C> {
    emitter: Emitter<'c, C>,
    last_emission_was_mempool: bool,
}

impl<'c, C: RpcApi> Iterator for UpdateIter<'c, C> {
    type Item = Result<EmittedUpdate, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.last_emission_was_mempool {
            self.last_emission_was_mempool = false;
            None
        } else {
            let update = self.emitter.emit_update();
            if matches!(update, Ok(EmittedUpdate::Mempool(_))) {
                self.last_emission_was_mempool = true;
            }
            Some(update)
        }
    }
}
