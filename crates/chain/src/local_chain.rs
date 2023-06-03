//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;

use crate::collections::BTreeMap;
use crate::{BlockId, ChainOracle};
use alloc::sync::Arc;
use bitcoin::BlockHash;

/// A structure that represents changes to [`LocalChain`].
pub type ChangeSet = BTreeMap<u32, Option<BlockHash>>;

/// Represents a block of [`LocalChain`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckPoint(Arc<CPInner>);

/// The internal contents of [`CheckPoint`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CPInner {
    /// Block id (hash and height).
    block: BlockId,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner>>,
}

/// Occurs when the caller contructs a [`CheckPoint`] with a height that is not higher than the
/// previous checkpoint it points to.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCheckPointError {
    /// The height of the new checkpoint.
    pub new_height: u32,
    /// The height of the previous checkpoint.
    pub prev_height: u32,
}

impl core::fmt::Display for NewCheckPointError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cannot construct checkpoint with a height ({}) that is not higher than the previous checkpoint ({})", self.new_height, self.prev_height)
    }
}

impl std::error::Error for NewCheckPointError {}

impl CheckPoint {
    /// Construct a [`CheckPoint`] from a [`BlockId`].
    pub fn new(block: BlockId) -> Self {
        Self(Arc::new(CPInner { block, prev: None }))
    }

    /// Construct a [`CheckPoint`] of `block` with a previous checkpoint.
    pub fn new_with_prev(
        block: BlockId,
        prev: Option<CheckPoint>,
    ) -> Result<Self, NewCheckPointError> {
        if let Some(prev_cp) = &prev {
            if prev_cp.height() >= block.height {
                return Err(NewCheckPointError {
                    new_height: block.height,
                    prev_height: prev_cp.height(),
                });
            }
        }

        Ok(Self(Arc::new(CPInner {
            block,
            prev: prev.map(|cp| cp.0),
        })))
    }

    /// Get the [`BlockId`] of the checkpoint.
    pub fn block_id(&self) -> BlockId {
        self.0.block
    }

    /// Get the height of the checkpoint.
    pub fn height(&self) -> u32 {
        self.0.block.height
    }

    /// Get the block hash of the checkpoint.
    pub fn hash(&self) -> BlockHash {
        self.0.block.hash
    }

    /// Detach this checkpoint from the previous.
    pub fn detach(self) -> Self {
        Self(Arc::new(CPInner {
            block: self.0.block,
            prev: None,
        }))
    }

    /// Get previous checkpoint.
    pub fn prev(&self) -> Option<CheckPoint> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate
    pub fn iter(&self) -> CheckPointIter {
        CheckPointIter {
            current: Some(Arc::clone(&self.0)),
        }
    }
}

/// A structure that iterates over checkpoints backwards.
pub struct CheckPointIter {
    current: Option<Arc<CPInner>>,
}

impl Iterator for CheckPointIter {
    type Item = CheckPoint;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.clone()?;
        self.current = current.prev.clone();
        Some(CheckPoint(current))
    }
}

/// This is a local implementation of [`ChainOracle`].
#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalChain {
    checkpoints: BTreeMap<u32, CheckPoint>,
}

impl From<LocalChain> for BTreeMap<u32, BlockHash> {
    fn from(value: LocalChain) -> Self {
        value
            .checkpoints
            .values()
            .map(|cp| (cp.height(), cp.hash()))
            .collect()
    }
}

impl From<ChangeSet> for LocalChain {
    fn from(value: ChangeSet) -> Self {
        Self::from_changeset(value)
    }
}

impl From<BTreeMap<u32, BlockHash>> for LocalChain {
    fn from(value: BTreeMap<u32, BlockHash>) -> Self {
        Self::from_blocks(value)
    }
}

impl ChainOracle for LocalChain {
    type Error = Infallible;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        chain_tip: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        if block.height > chain_tip.height {
            return Ok(None);
        }
        Ok(
            match (
                self.checkpoints.get(&block.height),
                self.checkpoints.get(&chain_tip.height),
            ) {
                (Some(cp), Some(tip_cp)) => {
                    Some(cp.hash() == block.hash && tip_cp.hash() == chain_tip.hash)
                }
                _ => None,
            },
        )
    }

    fn get_chain_tip(&self) -> Result<Option<BlockId>, Self::Error> {
        Ok(self.checkpoints.values().last().map(CheckPoint::block_id))
    }
}

impl LocalChain {
    /// Construct a [`LocalChain`] from an initial `changeset`.
    pub fn from_changeset(changeset: ChangeSet) -> Self {
        let mut chain = Self::default();
        chain.apply_changeset(&changeset);
        chain
    }

    /// Construct a [`LocalChain`] from a given `checkpoint` tip.
    pub fn from_checkpoint(checkpoint: CheckPoint) -> Self {
        Self {
            checkpoints: checkpoint.iter().map(|cp| (cp.height(), cp)).collect(),
        }
    }

    /// Constructs a [`LocalChain`] from a [`BTreeMap`] of height to [`BlockHash`].
    ///
    /// The [`BTreeMap`] enforces the height order. However, the caller must ensure the blocks are
    /// all of the same chain.
    pub fn from_blocks(blocks: BTreeMap<u32, BlockHash>) -> Self {
        Self {
            checkpoints: blocks
                .into_iter()
                .map({
                    let mut prev = None;
                    move |(height, hash)| {
                        let cp = CheckPoint::new_with_prev(BlockId { height, hash }, prev.clone())
                            .expect("must not fail");
                        prev = Some(cp.clone());
                        (height, cp)
                    }
                })
                .collect(),
        }
    }

    /// Get the highest checkpoint.
    pub fn tip(&self) -> Option<CheckPoint> {
        self.checkpoints.values().last().cloned()
    }

    /// Returns whether the [`LocalChain`] is empty (has no checkpoints).
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    /// Previews, and optionally applies updates to [`Self`] with the given `new_tip`.
    ///
    /// The method returns `(apply_update, changeset)` if [`Ok`]. `apply_update` is a closure that
    /// can be called to apply the changes represented in `changeset.
    ///
    /// To update, the `new_tip` must *connect* with `self`. If `self` and `new_tip` has a mutual
    /// checkpoint (same height and hash), it can connect if:
    /// * The mutual checkpoint is the tip of `self`.
    /// * An ancestor of `new_tip` has a height which is of the checkpoint one higher than the
    ///         mutual checkpoint from `self`.
    ///
    /// Additionally:
    /// * If `self` is empty, `new_tip` will always connect.
    /// * If `self` only has one checkpoint, `new_tip` must have an ancestor checkpoint with the
    ///     same height as it.
    ///
    /// To invalidate from a given checkpoint, `new_tip` must contain an ancestor checkpoint with
    /// the same height but different hash.
    ///
    /// # Errors
    ///
    /// An error will occur if the update does not correctly connect with `self`.
    ///
    /// Refer to [module-level documentation] for more.
    ///
    /// [module-level documentation]: crate::local_chain
    pub fn update(
        &mut self,
        new_tip: CheckPoint,
    ) -> Result<(impl FnOnce() + '_, ChangeSet), CannotConnectError> {
        let mut updated_cps = BTreeMap::<u32, CheckPoint>::new();
        let mut agreement_height = Option::<u32>::None;
        let mut complete_match = false;

        for cp in new_tip.iter() {
            let block = cp.block_id();
            let original_cp = self.checkpoints.get(&block.height);

            // if original block of height does not exist, or if the hash does not match we will
            // need to update the original checkpoint at that height
            if original_cp.map(CheckPoint::block_id) != Some(block) {
                updated_cps.insert(block.height, cp.clone());
            }

            if let Some(original_cp) = original_cp {
                // record the first agreement height
                if agreement_height.is_none() && original_cp.block_id() == block {
                    agreement_height = Some(block.height);
                }
                // break if the internal pointers of the checkpoints are the same
                if Arc::as_ptr(&original_cp.0) == Arc::as_ptr(&cp.0) {
                    complete_match = true;
                    break;
                }
            }
        }

        // Lower bound of the range to invalidate in `self`.
        let invalidate_lb = match agreement_height {
            // if there is no agreement, we invalidate all of the original chain
            None => u32::MIN,
            // if the agreement is at the update's tip, we don't need to invalidate
            Some(height) if height == new_tip.height() => u32::MAX,
            Some(height) => height + 1,
        };

        let changeset = {
            // Construct initial changeset of heights to invalidate in `self`.
            let mut changeset = self
                .checkpoints
                .range(invalidate_lb..)
                .map(|(&height, _)| (height, None))
                .collect::<ChangeSet>();

            // The height of the first block to invalidate (if any) must be represented in the `update`.
            if let Some(first_invalidated_height) = changeset.keys().next() {
                if !updated_cps.contains_key(first_invalidated_height) {
                    return Err(CannotConnectError {
                        try_include: self
                            .checkpoints
                            .get(first_invalidated_height)
                            .expect("checkpoint already exists")
                            .block_id(),
                    });
                }
            }

            changeset.extend(
                updated_cps
                    .iter()
                    .map(|(height, cp)| (*height, Some(cp.hash()))),
            );
            changeset
        };

        let apply_update = move || {
            if let Some(&start_height) = updated_cps.keys().next() {
                self.checkpoints.split_off(&invalidate_lb);
                self.checkpoints.append(&mut updated_cps);
                if !self.is_empty() && !complete_match {
                    self.fix_links(start_height);
                }
            }
        };

        Ok((apply_update, changeset))
    }

    /// Apply the given `changeset`.
    pub fn apply_changeset(&mut self, changeset: &ChangeSet) {
        if let Some(start_height) = changeset.keys().next().cloned() {
            for (&height, &hash) in changeset {
                match hash {
                    Some(hash) => self
                        .checkpoints
                        .insert(height, CheckPoint::new(BlockId { height, hash })),
                    None => self.checkpoints.remove(&height),
                };
            }
            self.fix_links(start_height);
        }
    }

    /// Update [`LocalChain`].
    ///
    /// This is equivalent to calling [`update`] and applying the update in sequence.
    ///
    /// [`update`]: Self::update
    pub fn apply_update(&mut self, new_tip: CheckPoint) -> Result<ChangeSet, CannotConnectError> {
        let (apply, changeset) = self.update(new_tip)?;
        apply();
        Ok(changeset)
    }

    /// Get or insert a `block_id`.
    ///
    /// # Errors
    ///
    /// Replacing the block hash of an existing checkpoint will result in an error.
    pub fn get_or_insert(
        &mut self,
        block_id: BlockId,
    ) -> Result<(CheckPoint, ChangeSet), InsertBlockError> {
        use crate::collections::btree_map::Entry;

        match self.checkpoints.entry(block_id.height) {
            Entry::Vacant(entry) => {
                entry.insert(CheckPoint::new(block_id));
                self.fix_links(block_id.height);
                let cp = self.checkpoint(block_id.height).expect("must be inserted");
                let changeset =
                    core::iter::once((block_id.height, Some(block_id.hash))).collect::<ChangeSet>();
                Ok((cp, changeset))
            }
            Entry::Occupied(entry) => {
                let cp = entry.get();
                if cp.block_id() == block_id {
                    Ok((cp.clone(), ChangeSet::default()))
                } else {
                    Err(InsertBlockError {
                        height: block_id.height,
                        original_hash: cp.hash(),
                        update_hash: block_id.hash,
                    })
                }
            }
        }
    }

    fn fix_links(&mut self, start_height: u32) {
        let mut prev = self
            .checkpoints
            .range(..start_height)
            .last()
            .map(|(_, cp)| cp.clone());

        for (_, cp) in self.checkpoints.range_mut(start_height..) {
            if cp.0.prev.as_ref().map(Arc::as_ptr) != prev.as_ref().map(|cp| Arc::as_ptr(&cp.0)) {
                cp.0 = Arc::new(CPInner {
                    block: cp.block_id(),
                    prev: prev.clone().map(|cp| cp.0),
                });
            }
            prev = Some(cp.clone());
        }
    }

    /// Derives an initial [`ChangeSet`], meaning that it can be applied to an empty chain to
    /// recover the current chain.
    pub fn initial_changeset(&self) -> ChangeSet {
        self.iter_checkpoints(None)
            .map(|cp| (cp.height(), Some(cp.hash())))
            .collect()
    }

    /// Get checkpoint of `height` (if any).
    pub fn checkpoint(&self, height: u32) -> Option<CheckPoint> {
        self.checkpoints.get(&height).cloned()
    }

    /// Iterate over checkpoints in decending height order.
    ///
    /// `height_upper_bound` is inclusive. A value of `None` means there is no bound, so all
    /// checkpoints will be traversed.
    pub fn iter_checkpoints(&self, height_upper_bound: Option<u32>) -> CheckPointIter {
        CheckPointIter {
            current: match height_upper_bound {
                Some(height) => self
                    .checkpoints
                    .range(..=height)
                    .last()
                    .map(|(_, cp)| cp.0.clone()),
                None => self.checkpoints.values().last().map(|cp| cp.0.clone()),
            },
        }
    }

    /// Get a reference to the internal checkpoint map.
    pub fn checkpoints(&self) -> &BTreeMap<u32, CheckPoint> {
        &self.checkpoints
    }
}

/// Represents a failure when trying to insert a checkpoint into [`LocalChain`].
#[derive(Clone, Debug, PartialEq)]
pub struct InsertBlockError {
    /// The checkpoints' height.
    pub height: u32,
    /// Original checkpoint's block hash.
    pub original_hash: BlockHash,
    /// Update checkpoint's block hash.
    pub update_hash: BlockHash,
}

impl core::fmt::Display for InsertBlockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "failed to insert block at height {} as blockhashes conflict: original={}, update={}",
            self.height, self.original_hash, self.update_hash
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InsertBlockError {}

/// Occurs when an update does not have a common checkpoint with the original chain.
#[derive(Clone, Debug, PartialEq)]
pub struct CannotConnectError {
    /// The suggested checkpoint to include to connect the two chains.
    pub try_include: BlockId,
}

impl core::fmt::Display for CannotConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "introduced chain cannot connect with the original chain, try include {}:{}",
            self.try_include.height, self.try_include.hash,
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CannotConnectError {}
