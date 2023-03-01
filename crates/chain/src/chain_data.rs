use bitcoin::{hashes::Hash, BlockHash, OutPoint, TxOut, Txid};

use crate::{
    sparse_chain::{self, ChainPosition},
    COINBASE_MATURITY,
};

/// Represents the height in which a transaction is confirmed at.
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

/// Block height and timestamp in which a transaction is confirmed in.
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub enum ConfirmationTime {
    Confirmed { height: u32, time: u64 },
    Unconfirmed,
}

impl sparse_chain::ChainPosition for ConfirmationTime {
    fn height(&self) -> TxHeight {
        match self {
            ConfirmationTime::Confirmed { height, .. } => TxHeight::Confirmed(*height),
            ConfirmationTime::Unconfirmed => TxHeight::Unconfirmed,
        }
    }

    fn max_ord_of_height(height: TxHeight) -> Self {
        match height {
            TxHeight::Confirmed(height) => Self::Confirmed {
                height,
                time: u64::MAX,
            },
            TxHeight::Unconfirmed => Self::Unconfirmed,
        }
    }

    fn min_ord_of_height(height: TxHeight) -> Self {
        match height {
            TxHeight::Confirmed(height) => Self::Confirmed {
                height,
                time: u64::MIN,
            },
            TxHeight::Unconfirmed => Self::Unconfirmed,
        }
    }
}

impl ConfirmationTime {
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }
}

/// A reference to a block in the cannonical chain.
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct BlockId {
    /// The height the block was confirmed at
    pub height: u32,
    /// The hash of the block
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

/// A `TxOut` with as much data as we can retreive about it
#[derive(Debug, Clone, PartialEq)]
pub struct FullTxOut<I> {
    /// The location of the `TxOut`
    pub outpoint: OutPoint,
    /// The `TxOut`
    pub txout: TxOut,
    /// The position of the transaction in `outpoint` in the overall chain.
    pub chain_position: I,
    /// The txid and chain position of the transaction (if any) that has spent this output.
    pub spent_by: Option<(I, Txid)>,
    /// Whether this output is on a coinbase transaction
    pub is_on_coinbase: bool,
}

impl<I: ChainPosition> FullTxOut<I> {
    /// Whether the utxo is/was/will be spendable at `height`.
    ///
    /// It is spendable if it is not an immature coinbase output and no spending tx has been
    /// confirmed by that heigt.
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

// TOOD: make test
