use crate::collections::{BTreeMap, BTreeSet};
use crate::COINBASE_MATURITY;
use alloc::vec::Vec;
use bitcoin::{hashes::Hash, BlockHash, OutPoint, TxOut, Txid};

/// Represents the observed position of some chain data.
///
/// The generic `A` should be a [`Anchor`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, core::hash::Hash)]
pub enum ChainPosition<A> {
    /// The chain data is seen as confirmed, and in anchored by `A`.
    Confirmed(A),
    /// The chain data is not confirmed and last seen in the mempool at this timestamp.
    Unconfirmed(u64),
}

impl<A> ChainPosition<A> {
    /// Returns whether [`ChainPosition`] is confirmed or not.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }
}

impl<A: Clone> ChainPosition<&A> {
    /// Maps a [`ChainPosition<&A>`] into a [`ChainPosition<A>`] by cloning the contents.
    pub fn cloned(self) -> ChainPosition<A> {
        match self {
            ChainPosition::Confirmed(a) => ChainPosition::Confirmed(a.clone()),
            ChainPosition::Unconfirmed(last_seen) => ChainPosition::Unconfirmed(last_seen),
        }
    }
}

impl<A: Anchor> ChainPosition<A> {
    /// Determines the upper bound of the confirmation height.
    pub fn confirmation_height_upper_bound(&self) -> Option<u32> {
        match self {
            ChainPosition::Confirmed(a) => Some(a.confirmation_height_upper_bound()),
            ChainPosition::Unconfirmed(_) => None,
        }
    }
}

/// Block height and timestamp at which a transaction is confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub enum ConfirmationTime {
    /// The transaction is confirmed
    Confirmed {
        /// Confirmation height.
        height: u32,
        /// Confirmation time in unix seconds.
        time: u64,
    },
    /// The transaction is unconfirmed
    Unconfirmed {
        /// The last-seen timestamp in unix seconds.
        last_seen: u64,
    },
}

impl ConfirmationTime {
    /// Construct an unconfirmed variant using the given `last_seen` time in unix seconds.
    pub fn unconfirmed(last_seen: u64) -> Self {
        Self::Unconfirmed { last_seen }
    }

    /// Returns whether [`ConfirmationTime`] is the confirmed variant.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }
}

impl From<ChainPosition<ConfirmationTimeHeightAnchor>> for ConfirmationTime {
    fn from(observed_as: ChainPosition<ConfirmationTimeHeightAnchor>) -> Self {
        match observed_as {
            ChainPosition::Confirmed(a) => Self::Confirmed {
                height: a.confirmation_height,
                time: a.confirmation_time,
            },
            ChainPosition::Unconfirmed(last_seen) => Self::Unconfirmed { last_seen },
        }
    }
}

/// A reference to a block in the canonical chain.
///
/// `BlockId` implements [`Anchor`]. When a transaction is anchored to `BlockId`, the confirmation
/// block and anchor block are the same block.
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct BlockId {
    /// The height of the block.
    pub height: u32,
    /// The hash of the block.
    pub hash: BlockHash,
}

impl Anchor for BlockId {
    fn anchor_block(&self) -> Self {
        *self
    }
}

impl AnchorFromBlockPosition for BlockId {
    fn from_block_position(_block: &bitcoin::Block, block_id: BlockId, _tx_pos: usize) -> Self {
        block_id
    }
}

impl Default for BlockId {
    fn default() -> Self {
        Self {
            height: Default::default(),
            hash: BlockHash::all_zeros(),
        }
    }
}

impl From<(u32, BlockHash)> for BlockId {
    fn from((height, hash): (u32, BlockHash)) -> Self {
        Self { height, hash }
    }
}

impl From<BlockId> for (u32, BlockHash) {
    fn from(block_id: BlockId) -> Self {
        (block_id.height, block_id.hash)
    }
}

impl From<(&u32, &BlockHash)> for BlockId {
    fn from((height, hash): (&u32, &BlockHash)) -> Self {
        Self {
            height: *height,
            hash: *hash,
        }
    }
}

/// An [`Anchor`] implementation that also records the exact confirmation height of the transaction.
///
/// Note that the confirmation block and the anchor block can be different here.
///
/// Refer to [`Anchor`] for more details.
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct ConfirmationHeightAnchor {
    /// The exact confirmation height of the transaction.
    ///
    /// It is assumed that this value is never larger than the height of the anchor block.
    pub confirmation_height: u32,
    /// The anchor block.
    pub anchor_block: BlockId,
}

impl Anchor for ConfirmationHeightAnchor {
    fn anchor_block(&self) -> BlockId {
        self.anchor_block
    }

    fn confirmation_height_upper_bound(&self) -> u32 {
        self.confirmation_height
    }
}

impl AnchorFromBlockPosition for ConfirmationHeightAnchor {
    fn from_block_position(_block: &bitcoin::Block, block_id: BlockId, _tx_pos: usize) -> Self {
        Self {
            anchor_block: block_id,
            confirmation_height: block_id.height,
        }
    }
}

/// An [`Anchor`] implementation that also records the exact confirmation time and height of the
/// transaction.
///
/// Note that the confirmation block and the anchor block can be different here.
///
/// Refer to [`Anchor`] for more details.
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct ConfirmationTimeHeightAnchor {
    /// The confirmation height of the transaction being anchored.
    pub confirmation_height: u32,
    /// The confirmation time of the transaction being anchored.
    pub confirmation_time: u64,
    /// The anchor block.
    pub anchor_block: BlockId,
}

impl Anchor for ConfirmationTimeHeightAnchor {
    fn anchor_block(&self) -> BlockId {
        self.anchor_block
    }

    fn confirmation_height_upper_bound(&self) -> u32 {
        self.confirmation_height
    }
}

impl AnchorFromBlockPosition for ConfirmationTimeHeightAnchor {
    fn from_block_position(block: &bitcoin::Block, block_id: BlockId, _tx_pos: usize) -> Self {
        Self {
            anchor_block: block_id,
            confirmation_height: block_id.height,
            confirmation_time: block.header.time as _,
        }
    }
}

/// A `TxOut` with as much data as we can retrieve about it
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FullTxOut<A> {
    /// The position of the transaction in `outpoint` in the overall chain.
    pub chain_position: ChainPosition<A>,
    /// The location of the `TxOut`.
    pub outpoint: OutPoint,
    /// The `TxOut`.
    pub txout: TxOut,
    /// The txid and chain position of the transaction (if any) that has spent this output.
    pub spent_by: Option<(ChainPosition<A>, Txid)>,
    /// Whether this output is on a coinbase transaction.
    pub is_on_coinbase: bool,
}

impl<A: Anchor> FullTxOut<A> {
    /// Whether the `txout` is considered mature.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_mature(&self, tip: u32) -> bool {
        if self.is_on_coinbase {
            let tx_height = match &self.chain_position {
                ChainPosition::Confirmed(anchor) => anchor.confirmation_height_upper_bound(),
                ChainPosition::Unconfirmed(_) => {
                    debug_assert!(false, "coinbase tx can never be unconfirmed");
                    return false;
                }
            };
            let age = tip.saturating_sub(tx_height);
            if age + 1 < COINBASE_MATURITY {
                return false;
            }
        }

        true
    }

    /// Whether the utxo is/was/will be spendable with chain `tip`.
    ///
    /// This method does not take into account the lock time.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_confirmed_and_spendable(&self, tip: u32) -> bool {
        if !self.is_mature(tip) {
            return false;
        }

        let confirmation_height = match &self.chain_position {
            ChainPosition::Confirmed(anchor) => anchor.confirmation_height_upper_bound(),
            ChainPosition::Unconfirmed(_) => return false,
        };
        if confirmation_height > tip {
            return false;
        }

        // if the spending tx is confirmed within tip height, the txout is no longer spendable
        if let Some((ChainPosition::Confirmed(spending_anchor), _)) = &self.spent_by {
            if spending_anchor.anchor_block().height <= tip {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn chain_position_ord() {
        let unconf1 = ChainPosition::<ConfirmationHeightAnchor>::Unconfirmed(10);
        let unconf2 = ChainPosition::<ConfirmationHeightAnchor>::Unconfirmed(20);
        let conf1 = ChainPosition::Confirmed(ConfirmationHeightAnchor {
            confirmation_height: 9,
            anchor_block: BlockId {
                height: 20,
                ..Default::default()
            },
        });
        let conf2 = ChainPosition::Confirmed(ConfirmationHeightAnchor {
            confirmation_height: 12,
            anchor_block: BlockId {
                height: 15,
                ..Default::default()
            },
        });

        assert!(unconf2 > unconf1, "higher last_seen means higher ord");
        assert!(unconf1 > conf1, "unconfirmed is higher ord than confirmed");
        assert!(
            conf2 > conf1,
            "confirmation_height is higher then it should be higher ord"
        );
    }
}

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
/// # use bdk_chain::ConfirmationHeightAnchor;
/// # use bdk_chain::ConfirmationTimeHeightAnchor;
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
///     tx.txid(),
///     BlockId {
///         height: 1,
///         hash: Hash::hash("first".as_bytes()),
///     },
/// );
///
/// // Insert `tx` into a `TxGraph` that uses `ConfirmationHeightAnchor` as the anchor type.
/// // This anchor records the anchor block and the confirmation height of the transaction.
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
///
/// // Insert `tx` into a `TxGraph` that uses `ConfirmationTimeHeightAnchor` as the anchor type.
/// // This anchor records the anchor block, the confirmation height and time of the transaction.
/// // When a transaction is anchored with `ConfirmationTimeHeightAnchor`, the anchor block and
/// // confirmation block can be different. However, the confirmation block cannot be higher than
/// // the anchor block and both blocks must be in the same chain for the anchor to be valid.
/// let mut graph_c = TxGraph::<ConfirmationTimeHeightAnchor>::default();
/// let _ = graph_c.insert_tx(tx.clone());
/// graph_c.insert_anchor(
///     tx.txid(),
///     ConfirmationTimeHeightAnchor {
///         anchor_block: BlockId {
///             height: 2,
///             hash: Hash::hash("third".as_bytes()),
///         },
///         confirmation_height: 1,
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
    fn append(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        BTreeMap::extend(self, other)
    }

    fn is_empty(&self) -> bool {
        BTreeMap::is_empty(self)
    }
}

impl<T: Ord> Append for BTreeSet<T> {
    fn append(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        BTreeSet::extend(self, other)
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