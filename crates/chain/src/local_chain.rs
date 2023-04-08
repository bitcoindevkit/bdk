use core::{convert::Infallible, ops::Deref};

use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use bitcoin::BlockHash;

use crate::{BlockId, ChainOracle};

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalChain {
    blocks: BTreeMap<u32, BlockHash>,
}

// [TODO] We need a cache/snapshot thing for chain oracle.
// * Minimize calls to remotes.
// * Can we cache it forever? Should we drop stuff?
// * Assume anything deeper than (i.e. 10) blocks won't be reorged.
// * Is this a cache on txs or block? or both?
// [TODO] Parents of children are confirmed if children are confirmed.
impl ChainOracle for LocalChain {
    type Error = Infallible;

    fn get_tip_in_best_chain(&self) -> Result<Option<BlockId>, Self::Error> {
        Ok(self
            .blocks
            .iter()
            .last()
            .map(|(&height, &hash)| BlockId { height, hash }))
    }

    fn get_block_in_best_chain(&self, height: u32) -> Result<Option<BlockHash>, Self::Error> {
        Ok(self.blocks.get(&height).cloned())
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
    fn from(blocks: BTreeMap<u32, BlockHash>) -> Self {
        Self { blocks }
    }
}

impl LocalChain {
    pub fn tip(&self) -> Option<BlockId> {
        self.blocks
            .iter()
            .last()
            .map(|(&height, &hash)| BlockId { height, hash })
    }

    /// This is like the sparsechain's logic, expect we must guarantee that all invalidated heights
    /// are to be re-filled.
    pub fn determine_changeset<U>(&self, update: &U) -> Result<ChangeSet, UpdateError>
    where
        U: AsRef<BTreeMap<u32, BlockHash>>,
    {
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
                return Err(UpdateError::NotConnected(first_invalid_height));
            }
        }

        let invalidated_heights = invalidate_from_height
            .into_iter()
            .flat_map(|from_height| self.blocks.range(from_height..).map(|(h, _)| h));

        // invalidated heights must all exist in the update
        let mut missing_heights = Vec::<u32>::new();
        for invalidated_height in invalidated_heights {
            if !update.contains_key(invalidated_height) {
                missing_heights.push(*invalidated_height);
            }
        }
        if !missing_heights.is_empty() {
            return Err(UpdateError::MissingHeightsInUpdate(missing_heights));
        }

        let mut changeset = BTreeMap::<u32, BlockHash>::new();
        for (height, new_hash) in update {
            let original_hash = self.blocks.get(height);
            if Some(new_hash) != original_hash {
                changeset.insert(*height, *new_hash);
            }
        }
        Ok(ChangeSet(changeset))
    }

    /// Applies the given `changeset`.
    pub fn apply_changeset(&mut self, mut changeset: ChangeSet) {
        self.blocks.append(&mut changeset.0)
    }

    /// Updates [`LocalChain`] with an update [`LocalChain`].
    ///
    /// This is equivalent to calling [`determine_changeset`] and [`apply_changeset`] in sequence.
    ///
    /// [`determine_changeset`]: Self::determine_changeset
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn apply_update(&mut self, update: Self) -> Result<ChangeSet, UpdateError> {
        let changeset = self.determine_changeset(&update)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    pub fn initial_changeset(&self) -> ChangeSet {
        ChangeSet(self.blocks.clone())
    }

    pub fn heights(&self) -> BTreeSet<u32> {
        self.blocks.keys().cloned().collect()
    }
}

/// This is the return value of [`determine_changeset`] and represents changes to [`LocalChain`].
///
/// [`determine_changeset`]: LocalChain::determine_changeset
#[derive(Debug, Default, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
pub struct ChangeSet(pub BTreeMap<u32, BlockHash>);

impl Deref for ChangeSet {
    type Target = BTreeMap<u32, BlockHash>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Represents an update failure of [`LocalChain`].
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateError {
    /// The update cannot be applied to the chain because the chain suffix it represents did not
    /// connect to the existing chain. This error case contains the checkpoint height to include so
    /// that the chains can connect.
    NotConnected(u32),
    /// If the update results in displacements of original blocks, the update should include all new
    /// block hashes that have displaced the original block hashes. This error case contains the
    /// heights of all missing block hashes in the update.
    MissingHeightsInUpdate(Vec<u32>),
}

impl core::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            UpdateError::NotConnected(heights) => write!(
                f,
                "the update cannot connect with the chain, try include blockhash at height {}",
                heights
            ),
            UpdateError::MissingHeightsInUpdate(missing_heights) => write!(
                f,
                "block hashes of these heights must be included in the update to succeed: {:?}",
                missing_heights
            ),
        }
    }
}
