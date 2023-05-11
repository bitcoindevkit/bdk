use bitcoin::{hashes::Hash, BlockHash, OutPoint, TxOut, Txid};

use crate::{
    sparse_chain::{self, ChainPosition},
    Anchor, COINBASE_MATURITY,
};

/// Represents an observation of some chain data.
///
/// The generic `A` should be a [`Anchor`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, core::hash::Hash)]
pub enum ObservedAs<A> {
    /// The chain data is seen as confirmed, and in anchored by `A`.
    Confirmed(A),
    /// The chain data is seen in mempool at this given timestamp.
    Unconfirmed(u64),
}

impl<A: Clone> ObservedAs<&A> {
    pub fn cloned(self) -> ObservedAs<A> {
        match self {
            ObservedAs::Confirmed(a) => ObservedAs::Confirmed(a.clone()),
            ObservedAs::Unconfirmed(last_seen) => ObservedAs::Unconfirmed(last_seen),
        }
    }
}

/// Represents the height at which a transaction is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub enum TxHeight {
    Confirmed(u32),
    Unconfirmed,
}

impl Default for TxHeight {
    fn default() -> Self {
        Self::Unconfirmed
    }
}

impl core::fmt::Display for TxHeight {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Confirmed(h) => core::write!(f, "confirmed_at({})", h),
            Self::Unconfirmed => core::write!(f, "unconfirmed"),
        }
    }
}

impl From<Option<u32>> for TxHeight {
    fn from(opt: Option<u32>) -> Self {
        match opt {
            Some(h) => Self::Confirmed(h),
            None => Self::Unconfirmed,
        }
    }
}

impl From<TxHeight> for Option<u32> {
    fn from(height: TxHeight) -> Self {
        match height {
            TxHeight::Confirmed(h) => Some(h),
            TxHeight::Unconfirmed => None,
        }
    }
}

impl crate::sparse_chain::ChainPosition for TxHeight {
    fn height(&self) -> TxHeight {
        *self
    }

    fn max_ord_of_height(height: TxHeight) -> Self {
        height
    }

    fn min_ord_of_height(height: TxHeight) -> Self {
        height
    }
}

impl TxHeight {
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
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
    Confirmed { height: u32, time: u64 },
    Unconfirmed { last_seen: u64 },
}

impl sparse_chain::ChainPosition for ConfirmationTime {
    fn height(&self) -> TxHeight {
        match self {
            ConfirmationTime::Confirmed { height, .. } => TxHeight::Confirmed(*height),
            ConfirmationTime::Unconfirmed { .. } => TxHeight::Unconfirmed,
        }
    }

    fn max_ord_of_height(height: TxHeight) -> Self {
        match height {
            TxHeight::Confirmed(height) => Self::Confirmed {
                height,
                time: u64::MAX,
            },
            TxHeight::Unconfirmed => Self::Unconfirmed { last_seen: 0 },
        }
    }

    fn min_ord_of_height(height: TxHeight) -> Self {
        match height {
            TxHeight::Confirmed(height) => Self::Confirmed {
                height,
                time: u64::MIN,
            },
            TxHeight::Unconfirmed => Self::Unconfirmed { last_seen: 0 },
        }
    }
}

impl ConfirmationTime {
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }
}

impl From<ObservedAs<ConfirmationTimeAnchor>> for ConfirmationTime {
    fn from(observed_as: ObservedAs<ConfirmationTimeAnchor>) -> Self {
        match observed_as {
            ObservedAs::Confirmed(a) => Self::Confirmed {
                height: a.confirmation_height,
                time: a.confirmation_time,
            },
            ObservedAs::Unconfirmed(_) => Self::Unconfirmed { last_seen: 0 },
        }
    }
}

/// A reference to a block in the canonical chain.
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

impl Default for BlockId {
    fn default() -> Self {
        Self {
            height: Default::default(),
            hash: BlockHash::from_inner([0u8; 32]),
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
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct ConfirmationHeightAnchor {
    /// The anchor block.
    pub anchor_block: BlockId,

    /// The exact confirmation height of the transaction.
    ///
    /// It is assumed that this value is never larger than the height of the anchor block.
    pub confirmation_height: u32,
}

impl Anchor for ConfirmationHeightAnchor {
    fn anchor_block(&self) -> BlockId {
        self.anchor_block
    }

    fn confirmation_height_upper_bound(&self) -> u32 {
        self.confirmation_height
    }
}

/// An [`Anchor`] implementation that also records the exact confirmation time and height of the
/// transaction.
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct ConfirmationTimeAnchor {
    /// The anchor block.
    pub anchor_block: BlockId,

    pub confirmation_height: u32,
    pub confirmation_time: u64,
}

impl Anchor for ConfirmationTimeAnchor {
    fn anchor_block(&self) -> BlockId {
        self.anchor_block
    }

    fn confirmation_height_upper_bound(&self) -> u32 {
        self.confirmation_height
    }
}
/// A `TxOut` with as much data as we can retrieve about it
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FullTxOut<P> {
    /// The location of the `TxOut`.
    pub outpoint: OutPoint,
    /// The `TxOut`.
    pub txout: TxOut,
    /// The position of the transaction in `outpoint` in the overall chain.
    pub chain_position: P,
    /// The txid and chain position of the transaction (if any) that has spent this output.
    pub spent_by: Option<(P, Txid)>,
    /// Whether this output is on a coinbase transaction.
    pub is_on_coinbase: bool,
}

impl<P: ChainPosition> FullTxOut<P> {
    /// Whether the utxo is/was/will be spendable at `height`.
    ///
    /// It is spendable if it is not an immature coinbase output and no spending tx has been
    /// confirmed by that height.
    pub fn is_spendable_at(&self, height: u32) -> bool {
        if !self.is_mature(height) {
            return false;
        }

        if self.chain_position.height() > TxHeight::Confirmed(height) {
            return false;
        }

        match &self.spent_by {
            Some((spending_height, _)) => spending_height.height() > TxHeight::Confirmed(height),
            None => true,
        }
    }

    pub fn is_mature(&self, height: u32) -> bool {
        if self.is_on_coinbase {
            let tx_height = match self.chain_position.height() {
                TxHeight::Confirmed(tx_height) => tx_height,
                TxHeight::Unconfirmed => {
                    debug_assert!(false, "coinbase tx can never be unconfirmed");
                    return false;
                }
            };
            let age = height.saturating_sub(tx_height);
            if age + 1 < COINBASE_MATURITY {
                return false;
            }
        }

        true
    }
}

impl<A: Anchor> FullTxOut<ObservedAs<A>> {
    /// Whether the `txout` is considered mature.
    ///
    /// This is the alternative version of [`is_mature`] which depends on `chain_position` being a
    /// [`ObservedAs<A>`] where `A` implements [`Anchor`].
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpretted confirmation count may be
    /// less than the actual value.
    ///
    /// [`is_mature`]: Self::is_mature
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_mature(&self, tip: u32) -> bool {
        if self.is_on_coinbase {
            let tx_height = match &self.chain_position {
                ObservedAs::Confirmed(anchor) => anchor.confirmation_height_upper_bound(),
                ObservedAs::Unconfirmed(_) => {
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
    /// This method does not take into account the locktime.
    ///
    /// This is the alternative version of [`is_spendable_at`] which depends on `chain_position`
    /// being a [`ObservedAs<A>`] where `A` implements [`Anchor`].
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpretted confirmation count may be
    /// less than the actual value.
    ///
    /// [`is_spendable_at`]: Self::is_spendable_at
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_confirmed_and_spendable(&self, tip: u32) -> bool {
        if !self.is_mature(tip) {
            return false;
        }

        let confirmation_height = match &self.chain_position {
            ObservedAs::Confirmed(anchor) => anchor.confirmation_height_upper_bound(),
            ObservedAs::Unconfirmed(_) => return false,
        };
        if confirmation_height > tip {
            return false;
        }

        // if the spending tx is confirmed within tip height, the txout is no longer spendable
        if let Some((ObservedAs::Confirmed(spending_anchor), _)) = &self.spent_by {
            if spending_anchor.anchor_block().height <= tip {
                return false;
            }
        }

        true
    }
}
