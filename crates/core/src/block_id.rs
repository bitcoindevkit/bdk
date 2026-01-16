use core::fmt::{self, Display};
use core::str::FromStr;

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

impl Display for BlockId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}", self.height, self.hash)
    }
}

impl FromStr for BlockId {
    type Err = ParseBlockIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (height_str, hash_str) = s.split_once(':').ok_or(ParseBlockIdError::InvalidFormat)?;

        let height = height_str
            .parse::<u32>()
            .map_err(|_| ParseBlockIdError::InvalidHeight)?;

        let hash = hash_str
            .parse::<BlockHash>()
            .map_err(|_| ParseBlockIdError::InvalidBlockhash)?;

        Ok(BlockId { height, hash })
    }
}

/// [`BlockId`] parsing errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseBlockIdError {
    /// Invalid [`BlockId`] representation (missing `:` separator).
    InvalidFormat,
    /// Invalid block height (failed to parse into a `u32`).
    InvalidHeight,
    /// Invalid block hash (failed to parse into a [`Blockhash`]).
    InvalidBlockhash,
}

impl Display for ParseBlockIdError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => write!(
                f,
                "Failed to parse string into `BlockId`, expected `<height>:<hash>`"
            ),
            Self::InvalidHeight => write!(f, "Failed to parse height into a u32."),
            Self::InvalidBlockhash => write!(f, "Failed to parse hash into a `Blockhash`."),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParseBlockIdError {}

/// Represents the confirmation block and time of a transaction.
#[derive(Debug, Default, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ConfirmationBlockTime {
    /// The anchor block.
    pub block_id: BlockId,
    /// The confirmation time of the transaction being anchored.
    pub confirmation_time: u64,
}
