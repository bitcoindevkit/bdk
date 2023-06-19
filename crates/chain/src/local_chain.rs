//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;

use alloc::collections::BTreeMap;
use bitcoin::BlockHash;

use crate::{BlockId, ChainOracle};

/// This is a local implementation of [`ChainOracle`].
#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalChain {
    blocks: BTreeMap<u32, BlockHash>,
}

impl ChainOracle for LocalChain {
    type Error = Infallible;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        static_block: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        if block.height > static_block.height {
            return Ok(None);
        }
        Ok(
            match (
                self.blocks.get(&block.height),
                self.blocks.get(&static_block.height),
            ) {
                (Some(&hash), Some(&static_hash)) => {
                    Some(hash == block.hash && static_hash == static_block.hash)
                }
                _ => None,
            },
        )
    }

    fn get_chain_tip(&self) -> Result<Option<BlockId>, Self::Error> {
        Ok(self.tip())
    }
}

impl AsRef<BTreeMap<u32, BlockHash>> for LocalChain {
    fn as_ref(&self) -> &BTreeMap<u32, BlockHash> {
        &self.blocks
    }
}

impl From<LocalChain> for BTreeMap<u32, BlockHash> {
    fn from(value: LocalChain) -> Self {
        value.blocks
    }
}

impl From<BTreeMap<u32, BlockHash>> for LocalChain {
    fn from(value: BTreeMap<u32, BlockHash>) -> Self {
        Self { blocks: value }
    }
}

impl LocalChain {
    /// Contruct a [`LocalChain`] from a list of [`BlockId`]s.
    pub fn from_blocks<B>(blocks: B) -> Self
    where
        B: IntoIterator<Item = BlockId>,
    {
        Self {
            blocks: blocks.into_iter().map(|b| (b.height, b.hash)).collect(),
        }
    }

    /// Get a reference to a map of block height to hash.
    pub fn blocks(&self) -> &BTreeMap<u32, BlockHash> {
        &self.blocks
    }

    /// Get the chain tip.
    pub fn tip(&self) -> Option<BlockId> {
        self.blocks
            .iter()
            .last()
            .map(|(&height, &hash)| BlockId { height, hash })
    }

    /// This is like the sparsechain's logic, expect we must guarantee that all invalidated heights
    /// are to be re-filled.
    pub fn determine_changeset(&self, update: &Self) -> Result<ChangeSet, UpdateNotConnectedError> {
        let update = update.as_ref();
        let update_tip = match update.keys().last().cloned() {
            Some(tip) => tip,
            None => return Ok(ChangeSet::default()),
        };

        // this is the latest height where both the update and local chain has the same block hash
        let agreement_height = update
            .iter()
            .rev()
            .find(|&(u_height, u_hash)| self.blocks.get(u_height) == Some(u_hash))
            .map(|(&height, _)| height);

        // the lower bound of the range to invalidate
        let invalidate_lb = match agreement_height {
            Some(height) if height == update_tip => u32::MAX,
            Some(height) => height + 1,
            None => 0,
        };

        // the first block's height to invalidate in the local chain
        let invalidate_from_height = self.blocks.range(invalidate_lb..).next().map(|(&h, _)| h);

        // the first block of height to invalidate (if any) should be represented in the update
        if let Some(first_invalid_height) = invalidate_from_height {
            if !update.contains_key(&first_invalid_height) {
                return Err(UpdateNotConnectedError(first_invalid_height));
            }
        }

        let mut changeset: BTreeMap<u32, Option<BlockHash>> = match invalidate_from_height {
            Some(first_invalid_height) => {
                // the first block of height to invalidate should be represented in the update
                if !update.contains_key(&first_invalid_height) {
                    return Err(UpdateNotConnectedError(first_invalid_height));
                }
                self.blocks
                    .range(first_invalid_height..)
                    .map(|(height, _)| (*height, None))
                    .collect()
            }
            None => BTreeMap::new(),
        };
        for (height, update_hash) in update {
            let original_hash = self.blocks.get(height);
            if Some(update_hash) != original_hash {
                changeset.insert(*height, Some(*update_hash));
            }
        }

        Ok(changeset)
    }

    /// Applies the given `changeset`.
    pub fn apply_changeset(&mut self, changeset: ChangeSet) {
        for (height, blockhash) in changeset {
            match blockhash {
                Some(blockhash) => self.blocks.insert(height, blockhash),
                None => self.blocks.remove(&height),
            };
        }
    }

    /// Updates [`LocalChain`] with an update [`LocalChain`].
    ///
    /// This is equivalent to calling [`determine_changeset`] and [`apply_changeset`] in sequence.
    ///
    /// [`determine_changeset`]: Self::determine_changeset
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn apply_update(&mut self, update: Self) -> Result<ChangeSet, UpdateNotConnectedError> {
        let changeset = self.determine_changeset(&update)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    /// Derives a [`ChangeSet`] that assumes that there are no preceding changesets.
    ///
    /// The changeset returned will record additions of all blocks included in [`Self`].
    pub fn initial_changeset(&self) -> ChangeSet {
        self.blocks
            .iter()
            .map(|(&height, &hash)| (height, Some(hash)))
            .collect()
    }

    /// Insert a block of [`BlockId`] into the [`LocalChain`].
    ///
    /// # Error
    ///
    /// If the insertion height already contains a block, and the block has a different blockhash,
    /// this will result in an [`InsertBlockNotMatchingError`].
    pub fn insert_block(
        &mut self,
        block_id: BlockId,
    ) -> Result<ChangeSet, InsertBlockNotMatchingError> {
        let mut update = Self::from_blocks(self.tip());

        if let Some(original_hash) = update.blocks.insert(block_id.height, block_id.hash) {
            if original_hash != block_id.hash {
                return Err(InsertBlockNotMatchingError {
                    height: block_id.height,
                    original_hash,
                    update_hash: block_id.hash,
                });
            }
        }

        Ok(self.apply_update(update).expect("should always connect"))
    }
}

/// This is the return value of [`determine_changeset`] and represents changes to [`LocalChain`].
///
/// [`determine_changeset`]: LocalChain::determine_changeset
pub type ChangeSet = BTreeMap<u32, Option<BlockHash>>;

/// Represents an update failure of [`LocalChain`] due to the update not connecting to the original
/// chain.
///
/// The update cannot be applied to the chain because the chain suffix it represents did not
/// connect to the existing chain. This error case contains the checkpoint height to include so
/// that the chains can connect.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateNotConnectedError(pub u32);

impl core::fmt::Display for UpdateNotConnectedError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "the update cannot connect with the chain, try include block at height {}",
            self.0
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for UpdateNotConnectedError {}

/// Represents a failure when trying to insert a checkpoint into [`LocalChain`].
#[derive(Clone, Debug, PartialEq)]
pub struct InsertBlockNotMatchingError {
    /// The checkpoints' height.
    pub height: u32,
    /// Original checkpoint's block hash.
    pub original_hash: BlockHash,
    /// Update checkpoint's block hash.
    pub update_hash: BlockHash,
}

impl core::fmt::Display for InsertBlockNotMatchingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "failed to insert block at height {} as blockhashes conflict: original={}, update={}",
            self.height, self.original_hash, self.update_hash
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InsertBlockNotMatchingError {}
