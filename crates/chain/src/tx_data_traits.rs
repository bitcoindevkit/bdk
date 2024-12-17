use crate::{BlockId, ConfirmationBlockTime};

/// Trait that "anchors" blockchain data to a specific block of height and hash.
///
/// If transaction A is anchored in block B, and block B is in the best chain, we can
/// assume that transaction A is also confirmed in the best chain. This does not necessarily mean
/// that transaction A is confirmed in block B. It could also mean transaction A is confirmed in a
/// parent block of B.
///
/// Every [`Anchor`] implementation must contain a [`BlockId`] parameter, and must implement
/// [`Ord`]. When implementing [`Ord`], the anchors' [`BlockId`]s should take precedence
/// over other elements inside the [`Anchor`]s for comparison purposes, i.e., you should first
/// compare the anchors' [`BlockId`]s and then care about the rest.
///
/// The example shows different types of anchors:
/// ```
/// # use bdk_chain::local_chain::LocalChain;
/// # use bdk_chain::tx_graph::TxGraph;
/// # use bdk_chain::BlockId;
/// # use bdk_chain::ConfirmationBlockTime;
/// # use bdk_chain::example_utils::*;
/// # use bitcoin::hashes::Hash;
/// // Initialize the local chain with two blocks.
/// let chain = LocalChain::from_blocks(
///     [
///         (1, Hash::hash("first".as_bytes())),
///         (2, Hash::hash("second".as_bytes())),
///     ]
///     .into_iter()
///     .collect(),
/// );
///
/// // Transaction to be inserted into `TxGraph`s with different anchor types.
/// let tx = tx_from_hex(RAW_TX_1);
///
/// // Insert `tx` into a `TxGraph` that uses `BlockId` as the anchor type.
/// // When a transaction is anchored with `BlockId`, the anchor block and the confirmation block of
/// // the transaction is the same block.
/// let mut graph_a = TxGraph::<BlockId>::default();
/// let _ = graph_a.insert_tx(tx.clone());
/// graph_a.insert_anchor(
///     tx.compute_txid(),
///     BlockId {
///         height: 1,
///         hash: Hash::hash("first".as_bytes()),
///     },
/// );
///
/// // Insert `tx` into a `TxGraph` that uses `ConfirmationBlockTime` as the anchor type.
/// // This anchor records the anchor block and the confirmation time of the transaction. When a
/// // transaction is anchored with `ConfirmationBlockTime`, the anchor block and confirmation block
/// // of the transaction is the same block.
/// let mut graph_c = TxGraph::<ConfirmationBlockTime>::default();
/// let _ = graph_c.insert_tx(tx.clone());
/// graph_c.insert_anchor(
///     tx.compute_txid(),
///     ConfirmationBlockTime {
///         block_id: BlockId {
///             height: 2,
///             hash: Hash::hash("third".as_bytes()),
///         },
///         confirmation_time: 123,
///     },
/// );
/// ```
pub trait Anchor: core::fmt::Debug + Clone + Eq + PartialOrd + Ord + core::hash::Hash {
    /// Returns the [`BlockId`] that the associated blockchain data is "anchored" in.
    fn anchor_block(&self) -> BlockId;

    /// Get the upper bound of the chain data's confirmation height.
    ///
    /// The default definition gives a pessimistic answer. This can be overridden by the `Anchor`
    /// implementation for a more accurate value.
    fn confirmation_height_upper_bound(&self) -> u32 {
        self.anchor_block().height
    }
}

impl<A: Anchor> Anchor for &A {
    fn anchor_block(&self) -> BlockId {
        <A as Anchor>::anchor_block(self)
    }
}

impl Anchor for BlockId {
    fn anchor_block(&self) -> Self {
        *self
    }
}

impl Anchor for ConfirmationBlockTime {
    fn anchor_block(&self) -> BlockId {
        self.block_id
    }

    fn confirmation_height_upper_bound(&self) -> u32 {
        self.block_id.height
    }
}

/// Set of parameters sufficient to construct an [`Anchor`].
///
/// Typically used as an additional constraint on anchor:
/// `for<'b> A: Anchor + From<TxPosInBlock<'b>>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxPosInBlock<'b> {
    /// Block in which the transaction appeared.
    pub block: &'b bitcoin::Block,
    /// Block's [`BlockId`].
    pub block_id: BlockId,
    /// Position in the block on which the transaction appeared.
    pub tx_pos: usize,
}

impl From<TxPosInBlock<'_>> for BlockId {
    fn from(pos: TxPosInBlock) -> Self {
        pos.block_id
    }
}

impl From<TxPosInBlock<'_>> for ConfirmationBlockTime {
    fn from(pos: TxPosInBlock) -> Self {
        Self {
            block_id: pos.block_id,
            confirmation_time: pos.block.header.time as _,
        }
    }
}
