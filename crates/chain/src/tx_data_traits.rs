use crate::collections::BTreeMap;
use crate::collections::BTreeSet;
use crate::BlockId;
use alloc::vec::Vec;

/// Trait that "anchors" blockchain data to a specific block of height and hash.
///
/// [`Anchor`] implementations must be [`Ord`] by the anchor block's [`BlockId`] first.
///
/// I.e. If transaction A is anchored in block B, then if block B is in the best chain, we can
/// assume that transaction A is also confirmed in the best chain. This does not necessarily mean
/// that transaction A is confirmed in block B. It could also mean transaction A is confirmed in a
/// parent block of B.
///
/// ```
/// # use bdk_chain::local_chain::LocalChain;
/// # use bdk_chain::tx_graph::TxGraph;
/// # use bdk_chain::BlockId;
/// # use bdk_chain::ConfirmationHeightAnchor;
/// # use bdk_chain::example_utils::*;
/// # use bitcoin::hashes::Hash;
///
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
///     tx.txid(),
///     BlockId {
///         height: 1,
///         hash: Hash::hash("first".as_bytes()),
///     },
/// );
///
/// // Insert `tx` into a `TxGraph` that uses `ConfirmationHeightAnchor` as the anchor type.
/// // When a transaction is anchored with `ConfirmationHeightAnchor`, the anchor block and
/// // confirmation block can be different. However, the confirmation block cannot be higher than
/// // the anchor block and both blocks must be in the same chain for the anchor to be valid.
/// let mut graph_b = TxGraph::<ConfirmationHeightAnchor>::default();
/// let _ = graph_b.insert_tx(tx.clone());
/// graph_b.insert_anchor(
///     tx.txid(),
///     ConfirmationHeightAnchor {
///         anchor_block: BlockId {
///             height: 2,
///             hash: Hash::hash("second".as_bytes()),
///         },
///         confirmation_height: 1,
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

impl<'a, A: Anchor> Anchor for &'a A {
    fn anchor_block(&self) -> BlockId {
        <A as Anchor>::anchor_block(self)
    }
}

/// An [`Anchor`] that can be constructed from a given block, block height and transaction position
/// within the block.
pub trait AnchorFromBlockPosition: Anchor {
    /// Construct the anchor from a given `block`, block height and `tx_pos` within the block.
    fn from_block_position(block: &bitcoin::Block, block_id: BlockId, tx_pos: usize) -> Self;
}

/// Trait that makes an object appendable.
pub trait Append {
    /// Append another object of the same type onto `self`.
    fn append(&mut self, other: Self);

    /// Returns whether the structure is considered empty.
    fn is_empty(&self) -> bool;
}

impl<K: Ord, V> Append for BTreeMap<K, V> {
    fn append(&mut self, mut other: Self) {
        BTreeMap::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        BTreeMap::is_empty(self)
    }
}

impl<T: Ord> Append for BTreeSet<T> {
    fn append(&mut self, mut other: Self) {
        BTreeSet::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        BTreeSet::is_empty(self)
    }
}

impl<T> Append for Vec<T> {
    fn append(&mut self, mut other: Self) {
        Vec::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }
}

impl<T> Append for Option<T> {
    // If other is Some then replace self's value with other's value, if other is None do nothing.
    fn append(&mut self, other: Self) {
        other.and_then(|v| self.replace(v));
    }

    fn is_empty(&self) -> bool {
        self.is_none()
    }
}

macro_rules! impl_append_for_tuple {
    ($($a:ident $b:tt)*) => {
        impl<$($a),*> Append for ($($a,)*) where $($a: Append),* {

            fn append(&mut self, _other: Self) {
                $(Append::append(&mut self.$b, _other.$b) );*
            }

            fn is_empty(&self) -> bool {
                $(Append::is_empty(&self.$b) && )* true
            }
        }
    }
}

impl_append_for_tuple!();
impl_append_for_tuple!(T0 0);
impl_append_for_tuple!(T0 0 T1 1);
impl_append_for_tuple!(T0 0 T1 1 T2 2);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7 T8 8);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7 T8 8 T9 9);
impl_append_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7 T8 8 T9 9 T10 10);
