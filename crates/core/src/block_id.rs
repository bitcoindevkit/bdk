use bitcoin::{hashes::Hash, BlockHash};

/// A reference to a block in the canonical chain.
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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

/// Represents the confirmation block and time of a transaction.
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ConfirmationBlockTime {
    /// The anchor block.
    pub block_id: BlockId,
    /// The confirmation time of the transaction being anchored.
    pub confirmation_time: u64,
}
