use bitcoin::{hashes::Hash, BlockHash, OutPoint, TxOut, Txid};

use crate::{Anchor, BlockTime, COINBASE_MATURITY};

/// Represents the observed position of some chain data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, core::hash::Hash)]
pub enum ChainPosition<A> {
    /// The chain data is seen as confirmed, and in anchored by `Anchor`.
    Confirmed(Anchor, A),
    /// The chain data is not confirmed and last seen in the mempool at this timestamp.
    Unconfirmed(u64),
}

impl<A> ChainPosition<A> {
    /// Returns whether [`ChainPosition`] is confirmed or not.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_, _))
    }
}

impl<A: Clone> ChainPosition<A> {
    /// Maps a [`ChainPosition`] into a [`ChainPosition`] by cloning the contents.
    pub fn cloned(self) -> ChainPosition<A> {
        match self {
            ChainPosition::Confirmed(anchor, a) => ChainPosition::Confirmed(anchor, a),
            ChainPosition::Unconfirmed(last_seen) => ChainPosition::Unconfirmed(last_seen),
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

impl From<ChainPosition<BlockTime>> for ConfirmationTime {
    fn from(observed_as: ChainPosition<BlockTime>) -> Self {
        match observed_as {
            ChainPosition::Confirmed((_txid, blockid), anchor_meta) => Self::Confirmed {
                height: blockid.height,
                time: *anchor_meta.as_ref() as u64,
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

impl<A> FullTxOut<A> {
    /// Whether the `txout` is considered mature.
    pub fn is_mature(&self, tip: u32) -> bool {
        if self.is_on_coinbase {
            let tx_height = match &self.chain_position {
                ChainPosition::Confirmed((_, blockid), _) => blockid.height,
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
    pub fn is_confirmed_and_spendable(&self, tip: u32) -> bool {
        if !self.is_mature(tip) {
            return false;
        }

        let confirmation_height = match &self.chain_position {
            ChainPosition::Confirmed((_, blockid), _) => blockid.height,
            ChainPosition::Unconfirmed(_) => return false,
        };
        if confirmation_height > tip {
            return false;
        }

        // if the spending tx is confirmed within tip height, the txout is no longer spendable
        if let Some((ChainPosition::Confirmed((_, spending_blockid), _), _)) = &self.spent_by {
            if spending_blockid.height <= tip {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::BlockTime;

    #[test]
    fn chain_position_ord() {
        let unconf1 = ChainPosition::Unconfirmed(10);
        let unconf2 = ChainPosition::Unconfirmed(20);
        let conf1 = ChainPosition::Confirmed(
            (
                Txid::all_zeros(),
                BlockId {
                    height: 9,
                    ..Default::default()
                },
            ),
            BlockTime::new(20),
        );
        let conf2 = ChainPosition::Confirmed(
            (
                Txid::all_zeros(),
                BlockId {
                    height: 12,
                    ..Default::default()
                },
            ),
            BlockTime::new(15),
        );

        assert!(unconf2 > unconf1, "higher last_seen means higher ord");
        assert!(unconf1 > conf1, "unconfirmed is higher ord than confirmed");
        assert!(
            conf2 > conf1,
            "confirmation_height is higher then it should be higher ord"
        );
    }
}
