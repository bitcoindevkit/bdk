use bitcoin::{hashes::Hash, BlockHash, OutPoint, TxOut, Txid};

use crate::{Anchor, AnchorFromBlockPosition, COINBASE_MATURITY};

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

impl From<ChainPosition<ConfirmationBlockTime>> for ConfirmationTime {
    fn from(observed_as: ChainPosition<ConfirmationBlockTime>) -> Self {
        match observed_as {
            ChainPosition::Confirmed(a) => Self::Confirmed {
                height: a.block_id.height,
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

/// An [`Anchor`] implementation that also records the exact confirmation time of the transaction.
///
/// Refer to [`Anchor`] for more details.
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct ConfirmationBlockTime {
    /// The anchor block.
    pub block_id: BlockId,
    /// The confirmation time of the transaction being anchored.
    pub confirmation_time: u64,
}

impl Anchor for ConfirmationBlockTime {
    fn anchor_block(&self) -> BlockId {
        self.block_id
    }

    fn confirmation_height_upper_bound(&self) -> u32 {
        self.block_id.height
    }
}

impl AnchorFromBlockPosition for ConfirmationBlockTime {
    fn from_block_position(block: &bitcoin::Block, block_id: BlockId, _tx_pos: usize) -> Self {
        Self {
            block_id,
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
        let unconf1 = ChainPosition::<ConfirmationBlockTime>::Unconfirmed(10);
        let unconf2 = ChainPosition::<ConfirmationBlockTime>::Unconfirmed(20);
        let conf1 = ChainPosition::Confirmed(ConfirmationBlockTime {
            confirmation_time: 20,
            block_id: BlockId {
                height: 9,
                ..Default::default()
            },
        });
        let conf2 = ChainPosition::Confirmed(ConfirmationBlockTime {
            confirmation_time: 15,
            block_id: BlockId {
                height: 12,
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
