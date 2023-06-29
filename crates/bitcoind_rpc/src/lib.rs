//! This crate is used for updating [`bdk_chain`] structures with data from the `bitcoind` RPC
//! interface.

#![warn(missing_docs)]

use bdk_chain::{
    bitcoin::{Block, Transaction, Txid},
    keychain::LocalUpdate,
    local_chain::CheckPoint,
    BlockId, ConfirmationHeightAnchor, ConfirmationTimeAnchor, TxGraph,
};
pub use bitcoincore_rpc;
use bitcoincore_rpc::{bitcoincore_rpc_json::GetBlockResult, Client, RpcApi};
use std::collections::HashSet;

/// An update emitted from [`BitcoindRpcEmitter`]. This can either be of a block or a subset of
/// mempool transactions.
#[derive(Debug, Clone)]
pub enum BitcoindRpcUpdate {
    /// An emitted block.
    Block {
        /// The checkpoint constructed from the block's height/hash and connected to the previous
        /// block.
        cp: CheckPoint,
        /// The result obtained from the `getblock` RPC call of this block's hash.
        info: Box<GetBlockResult>,
        ///
        block: Box<Block>,
    },
    /// An emitted subset of mempool transactions.
    ///
    /// [`BitcoindRpcEmitter`] attempts to avoid re-emitting transactions.
    Mempool {
        /// The checkpoint of the last-seen tip.
        cp: CheckPoint,
        /// Subset of mempool transactions.
        txs: Vec<(Transaction, u64)>,
    },
}

/// A closure that transforms a [`BitcoindRpcUpdate`] into a [`ConfirmationHeightAnchor`].
///
/// This is to be used as an input to [`BitcoindRpcUpdate::into_update`].
pub fn confirmation_height_anchor(
    info: &GetBlockResult,
    _txid: Txid,
    _tx_pos: usize,
) -> ConfirmationHeightAnchor {
    ConfirmationHeightAnchor {
        anchor_block: BlockId {
            height: info.height as _,
            hash: info.hash,
        },
        confirmation_height: info.height as _,
    }
}

/// A closure that transforms a [`BitcoindRpcUpdate`] into a [`ConfirmationTimeAnchor`].
///
/// This is to be used as an input to [`BitcoindRpcUpdate::into_update`].
pub fn confirmation_time_anchor(
    info: &GetBlockResult,
    _txid: Txid,
    _tx_pos: usize,
) -> ConfirmationTimeAnchor {
    ConfirmationTimeAnchor {
        anchor_block: BlockId {
            height: info.height as _,
            hash: info.hash,
        },
        confirmation_height: info.height as _,
        confirmation_time: info.time as _,
    }
}

impl BitcoindRpcUpdate {
    /// Returns whether the update is of a subset of the mempool.
    pub fn is_mempool(&self) -> bool {
        matches!(self, Self::Mempool { .. })
    }

    /// Returns whether the update is of a block.
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    /// Transforms the [`BitcoindRpcUpdate`] into a [`LocalUpdate`].
    ///
    /// [`confirmation_height_anchor`] and [`confirmation_time_anchor`] can be used as the `anchor`
    /// intput to construct updates with [`ConfirmationHeightAnchor`]s and
    /// [`ConfirmationTimeAnchor`]s respectively.
    pub fn into_update<K, A, F>(self, anchor: F) -> LocalUpdate<K, A>
    where
        A: Clone + Ord + PartialOrd,
        F: Fn(&GetBlockResult, Txid, usize) -> A,
    {
        match self {
            BitcoindRpcUpdate::Block { cp, info, block } => LocalUpdate {
                graph: {
                    let mut g = TxGraph::<A>::new(block.txdata);
                    for (tx_pos, &txid) in info.tx.iter().enumerate() {
                        let _ = g.insert_anchor(txid, anchor(&info, txid, tx_pos));
                    }
                    g
                },
                ..LocalUpdate::new(cp)
            },
            BitcoindRpcUpdate::Mempool { cp, txs } => LocalUpdate {
                graph: {
                    let mut last_seens = Vec::<(Txid, u64)>::with_capacity(txs.len());
                    let mut g = TxGraph::<A>::new(txs.into_iter().map(|(tx, last_seen)| {
                        last_seens.push((tx.txid(), last_seen));
                        tx
                    }));
                    for (txid, seen_at) in last_seens {
                        let _ = g.insert_seen_at(txid, seen_at);
                    }
                    g
                },
                ..LocalUpdate::new(cp)
            },
        }
    }
}

/// A structure that emits updates for [`bdk_chain`] structures, sourcing blockchain data from
/// [`bitcoincore_rpc::Client`].
///
/// Updates are of type [`BitcoindRpcUpdate`], where each update can either be of a whole block, or
/// a subset of the mempool.
///
/// A [`BitcoindRpcEmitter`] emits updates starting from the `fallback_height` provided in [`new`],
/// or if `last_cp` is provided, we start from the height above the agreed-upon blockhash (between
/// `last_cp` and the state of `bitcoind`). Blocks are emitted in sequence (ascending order), and
/// the mempool contents emitted if the last emission is the chain tip.
///
/// # [`Iterator`] implementation
///
/// [`BitcoindRpcEmitter`] implements [`Iterator`] in a way such that even after [`Iterator::next`]
/// returns [`None`], subsequent calls may resume returning [`Some`].
///
/// Returning [`None`] means that the previous call to [`next`] is the mempool. This is useful if
/// the caller wishes to update once.
///
/// ```rust,no_run
/// use bdk_bitcoind_rpc::{BitcoindRpcEmitter, BitcoindRpcUpdate};
/// # let client = todo!();
///
/// for update in BitcoindRpcEmitter::new(&client, 709_632, None) {
///     match update.expect("todo: deal with the error properly") {
///         BitcoindRpcUpdate::Block { cp, .. } => println!("block {}:{}", cp.height(), cp.hash()),
///         BitcoindRpcUpdate::Mempool { .. } => println!("mempool"),
///     }
/// }
/// ```
///
/// Alternatively, if the caller wishes to keep [`BitcoindRpcEmitter`] in a dedicated update-thread,
/// the caller can continue to poll [`next`] (potentially with a delay).
///
/// [`new`]: BitcoindRpcEmitter::new
/// [`next`]: Iterator::next
pub struct BitcoindRpcEmitter<'a> {
    client: &'a Client,
    fallback_height: u32,

    last_cp: Option<CheckPoint>,
    last_info: Option<GetBlockResult>,

    seen_txids: HashSet<Txid>,
    last_emission_was_mempool: bool,
}

impl<'a> Iterator for BitcoindRpcEmitter<'a> {
    /// Represents an emitted item.
    type Item = Result<BitcoindRpcUpdate, bitcoincore_rpc::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.last_emission_was_mempool {
            self.last_emission_was_mempool = false;
            None
        } else {
            Some(self.next_update())
        }
    }
}

impl<'a> BitcoindRpcEmitter<'a> {
    /// Constructs a new [`BitcoindRpcEmitter`] with the provided [`bitcoincore_rpc::Client`].
    ///
    /// * `fallback_height` is the block height to start from if `last_cp` is not provided, or a
    ///     point of agreement is not found.
    /// * `last_cp` is the last known checkpoint to build updates on (if any).
    pub fn new(client: &'a Client, fallback_height: u32, last_cp: Option<CheckPoint>) -> Self {
        Self {
            client,
            fallback_height,
            last_cp,
            last_info: None,
            seen_txids: HashSet::new(),
            last_emission_was_mempool: false,
        }
    }

    /// Continuously poll [`bitcoincore_rpc::Client`] until an update is found.
    pub fn next_update(&mut self) -> Result<BitcoindRpcUpdate, bitcoincore_rpc::Error> {
        loop {
            match self.poll()? {
                Some(item) => return Ok(item),
                None => continue,
            };
        }
    }

    /// Performs a single round of polling [`bitcoincore_rpc::Client`] and updating the internal
    /// state. This returns [`Ok(Some(BitcoindRpcUpdate))`] if an update is found.
    pub fn poll(&mut self) -> Result<Option<BitcoindRpcUpdate>, bitcoincore_rpc::Error> {
        let client = self.client;
        self.last_emission_was_mempool = false;

        match (&mut self.last_cp, &mut self.last_info) {
            // If `last_cp` and `last_info` are both none, we need to emit from the
            // `fallback_height`. `last_cp` and `last_info` will both be updated to the emitted
            // block.
            (last_cp @ None, last_info @ None) => {
                let info =
                    client.get_block_info(&client.get_block_hash(self.fallback_height as _)?)?;
                let block = self.client.get_block(&info.hash)?;
                let cp = CheckPoint::new(BlockId {
                    height: info.height as _,
                    hash: info.hash,
                });
                *last_cp = Some(cp.clone());
                *last_info = Some(info.clone());
                Ok(Some(BitcoindRpcUpdate::Block {
                    cp,
                    info: Box::new(info),
                    block: Box::new(block),
                }))
            }
            // If `last_cp` exists, but `last_info` does not, it means we have not fetched a
            // block from the client yet, but we have a previous checkpoint which we can use to
            // find the point of agreement with.
            //
            // We don't emit in this match case. Instead, we set the state to either:
            // * { last_cp: Some, last_info: Some } : When we find a point of agreement.
            // * { last_cp: None, last_indo: None } : When we cannot find a point of agreement.
            (last_cp @ Some(_), last_info @ None) => {
                for cp in last_cp.clone().iter().flat_map(CheckPoint::iter) {
                    let cp_block = cp.block_id();

                    let info = client.get_block_info(&cp_block.hash)?;
                    if info.confirmations < 0 {
                        // block is not in the main chain
                        continue;
                    }
                    // agreement found
                    *last_cp = Some(cp);
                    *last_info = Some(info);
                    return Ok(None);
                }

                // no point of agreement found, next call will emit block @ fallback height
                *last_cp = None;
                *last_info = None;
                Ok(None)
            }
            // If `last_cp` and `last_info` is both `Some`, we either emit a block at
            // `last_info.nextblockhash` (if it exists), or we emit a subset of the mempool.
            (Some(last_cp), last_info @ Some(_)) => {
                // find next block
                match last_info.as_ref().unwrap().nextblockhash {
                    Some(next_hash) => {
                        let info = self.client.get_block_info(&next_hash)?;

                        if info.confirmations < 0 {
                            *last_info = None;
                            return Ok(None);
                        }

                        let block = self.client.get_block(&info.hash)?;
                        let cp = last_cp
                            .clone()
                            .extend(BlockId {
                                height: info.height as _,
                                hash: info.hash,
                            })
                            .expect("must extend from checkpoint");

                        *last_cp = cp.clone();
                        *last_info = Some(info.clone());

                        Ok(Some(BitcoindRpcUpdate::Block {
                            cp,
                            info: Box::new(info),
                            block: Box::new(block),
                        }))
                    }
                    None => {
                        let mempool_txs = client
                            .get_raw_mempool()?
                            .into_iter()
                            .filter(|&txid| self.seen_txids.insert(txid))
                            .map(
                                |txid| -> Result<(Transaction, u64), bitcoincore_rpc::Error> {
                                    let first_seen =
                                        client.get_mempool_entry(&txid).map(|entry| entry.time)?;
                                    let tx = client.get_raw_transaction(&txid, None)?;
                                    Ok((tx, first_seen))
                                },
                            )
                            .collect::<Result<Vec<_>, _>>()?;

                        // After a mempool emission, we want to find the point of agreement in
                        // the next round.
                        *last_info = None;

                        self.last_emission_was_mempool = true;
                        Ok(Some(BitcoindRpcUpdate::Mempool {
                            txs: mempool_txs,
                            cp: last_cp.clone(),
                        }))
                    }
                }
            }
            (None, Some(info)) => unreachable!("got info with no checkpoint? info={:#?}", info),
        }
    }
}

/// Extends [`bitcoincore_rpc::Error`].
pub trait BitcoindRpcErrorExt {
    /// Returns whether the error is a "not found" error.
    ///
    /// This is useful since [`BitcoindRpcEmitter`] emits [`Result<_, bitcoincore_rpc::Error>`]s as
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
