//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;

use crate::collections::BTreeMap;
use crate::{BlockId, ChainOracle};
use alloc::sync::Arc;
use bitcoin::BlockHash;

/// A structure that represents changes to [`LocalChain`].
pub type ChangeSet = BTreeMap<u32, Option<BlockHash>>;

/// A [`LocalChain`] checkpoint is used to find the agreement point between two chains and as a
/// transaction anchor.
///
/// Each checkpoint contains the height and hash of a block ([`BlockId`]).
///
/// Internaly, checkpoints are nodes of a linked-list. This allows the caller to view the entire
/// chain without holding a lock to [`LocalChain`].
#[derive(Debug, Clone)]
pub struct CheckPoint(Arc<CPInner>);

/// The internal contents of [`CheckPoint`].
#[derive(Debug, Clone)]
struct CPInner {
    /// Block id (hash and height).
    block: BlockId,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner>>,
}

impl CheckPoint {
    /// Construct a new base block at the front of a linked list.
    pub fn new(block: BlockId) -> Self {
        Self(Arc::new(CPInner { block, prev: None }))
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the one you
    /// are pushing on to.
    pub fn push(self, block: BlockId) -> Result<Self, Self> {
        if self.height() < block.height {
            Ok(Self(Arc::new(CPInner {
                block,
                prev: Some(self.0),
            })))
        } else {
            Err(self)
        }
    }

    /// Extends the checkpoint linked list by a iterator of block ids.
    ///
    /// Returns an `Err(self)` if there is block which does not have a greater height than the
    /// previous one.
    pub fn extend_with_blocks(
        self,
        blocks: impl IntoIterator<Item = BlockId>,
    ) -> Result<Self, Self> {
        let mut curr = self.clone();
        for block in blocks {
            curr = curr.push(block).map_err(|_| self.clone())?;
        }
        Ok(curr)
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

    /// Get the previous checkpoint in the chain
    pub fn prev(&self) -> Option<CheckPoint> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter {
        self.clone().into_iter()
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

impl IntoIterator for CheckPoint {
    type Item = CheckPoint;
    type IntoIter = CheckPointIter;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter {
            current: Some(self.0),
        }
    }
}

/// A struct to update [`LocalChain`].
///
/// This is used as input for [`LocalChain::apply_update`]. It contains the update's chain `tip` and
/// a `bool` which signals whether this update can introduce blocks below the original chain's tip
/// without invalidating blocks residing on the original chain. Block-by-block syncing mechanisms
/// would typically create updates that builds upon the previous tip. In this case, this paramater
/// would be `false`. Script-pubkey based syncing mechanisms may not introduce transactions in a
/// chronological order so some updates require introducing older blocks (to anchor older
/// transactions). For script-pubkey based syncing, this parameter would typically be `true`.
#[derive(Debug, Clone)]
pub struct Update {
    /// The update chain's new tip.
    pub tip: CheckPoint,

    /// Whether the update allows for introducing older blocks.
    ///
    /// Refer to [struct-level documentation] for more.
    ///
    /// [struct-level documentation]: Update
    pub introduce_older_blocks: bool,
}

/// This is a local implementation of [`ChainOracle`].
#[derive(Debug, Default, Clone)]
pub struct LocalChain {
    tip: Option<CheckPoint>,
    index: BTreeMap<u32, BlockHash>,
}

impl PartialEq for LocalChain {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl From<LocalChain> for BTreeMap<u32, BlockHash> {
    fn from(value: LocalChain) -> Self {
        value.index
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
                self.index.get(&block.height),
                self.index.get(&chain_tip.height),
            ) {
                (Some(cp), Some(tip_cp)) => Some(*cp == block.hash && *tip_cp == chain_tip.hash),
                _ => None,
            },
        )
    }

    fn get_chain_tip(&self) -> Result<Option<BlockId>, Self::Error> {
        Ok(self.tip.as_ref().map(|tip| tip.block_id()))
    }
}

impl LocalChain {
    /// Construct a [`LocalChain`] from an initial `changeset`.
    pub fn from_changeset(changeset: ChangeSet) -> Self {
        let mut chain = Self::default();
        chain.apply_changeset(&changeset);

        debug_assert!(chain._check_index_is_consistent_with_tip());
        debug_assert!(chain._check_changeset_is_applied(&changeset));

        chain
    }

    /// Construct a [`LocalChain`] from a given `checkpoint` tip.
    pub fn from_tip(tip: CheckPoint) -> Self {
        let mut _self = Self {
            tip: Some(tip),
            ..Default::default()
        };
        _self.reindex(0);
        debug_assert!(_self._check_index_is_consistent_with_tip());
        _self
    }

    /// Constructs a [`LocalChain`] from a [`BTreeMap`] of height to [`BlockHash`].
    ///
    /// The [`BTreeMap`] enforces the height order. However, the caller must ensure the blocks are
    /// all of the same chain.
    pub fn from_blocks(blocks: BTreeMap<u32, BlockHash>) -> Self {
        let mut tip: Option<CheckPoint> = None;

        for block in &blocks {
            match tip {
                Some(curr) => {
                    tip = Some(
                        curr.push(BlockId::from(block))
                            .expect("BTreeMap is ordered"),
                    )
                }
                None => tip = Some(CheckPoint::new(BlockId::from(block))),
            }
        }

        let chain = Self { index: blocks, tip };

        debug_assert!(chain._check_index_is_consistent_with_tip());

        chain
    }

    /// Get the highest checkpoint.
    pub fn tip(&self) -> Option<CheckPoint> {
        self.tip.clone()
    }

    /// Returns whether the [`LocalChain`] is empty (has no checkpoints).
    pub fn is_empty(&self) -> bool {
        self.tip.is_none()
    }

    /// Applies the given `update` to the chain.
    ///
    /// The method returns [`ChangeSet`] on success. This represents the applied changes to `self`.
    ///
    /// To update, the `update_tip` must *connect* with `self`. If `self` and `update_tip` has a
    /// mutual checkpoint (same height and hash), it can connect if:
    /// * The mutual checkpoint is the tip of `self`.
    /// * An ancestor of `update_tip` has a height which is of the checkpoint one higher than the
    ///         mutual checkpoint from `self`.
    ///
    /// Additionally:
    /// * If `self` is empty, `update_tip` will always connect.
    /// * If `self` only has one checkpoint, `update_tip` must have an ancestor checkpoint with the
    ///     same height as it.
    ///
    /// To invalidate from a given checkpoint, `update_tip` must contain an ancestor checkpoint with
    /// the same height but different hash.
    ///
    /// # Errors
    ///
    /// An error will occur if the update does not correctly connect with `self`.
    ///
    /// Refer to [`Update`] for more about the update struct.
    ///
    /// [module-level documentation]: crate::local_chain
    pub fn apply_update(&mut self, update: Update) -> Result<ChangeSet, CannotConnectError> {
        match self.tip() {
            Some(original_tip) => {
                let changeset = merge_chains(
                    original_tip,
                    update.tip.clone(),
                    update.introduce_older_blocks,
                )?;
                self.apply_changeset(&changeset);

                // return early as `apply_changeset` already calls `check_consistency`
                Ok(changeset)
            }
            None => {
                *self = Self::from_tip(update.tip);
                let changeset = self.initial_changeset();

                debug_assert!(self._check_index_is_consistent_with_tip());
                debug_assert!(self._check_changeset_is_applied(&changeset));
                Ok(changeset)
            }
        }
    }

    /// Apply the given `changeset`.
    pub fn apply_changeset(&mut self, changeset: &ChangeSet) {
        if let Some(start_height) = changeset.keys().next().cloned() {
            let mut extension = BTreeMap::default();
            let mut base: Option<CheckPoint> = None;
            for cp in self.iter_checkpoints() {
                if cp.height() >= start_height {
                    extension.insert(cp.height(), cp.hash());
                } else {
                    base = Some(cp);
                    break;
                }
            }

            for (&height, &hash) in changeset {
                match hash {
                    Some(hash) => {
                        extension.insert(height, hash);
                    }
                    None => {
                        extension.remove(&height);
                    }
                };
            }
            let new_tip = match base {
                Some(base) => Some(
                    base.extend_with_blocks(extension.into_iter().map(BlockId::from))
                        .expect("extension is strictly greater than base"),
                ),
                None => LocalChain::from_blocks(extension).tip(),
            };
            self.tip = new_tip;
            self.reindex(start_height);

            debug_assert!(self._check_index_is_consistent_with_tip());
            debug_assert!(self._check_changeset_is_applied(changeset));
        }
    }

    /// Insert a [`BlockId`].
    ///
    /// # Errors
    ///
    /// Replacing the block hash of an existing checkpoint will result in an error.
    pub fn insert_block(&mut self, block_id: BlockId) -> Result<ChangeSet, InsertBlockError> {
        if let Some(&original_hash) = self.index.get(&block_id.height) {
            if original_hash != block_id.hash {
                return Err(InsertBlockError {
                    height: block_id.height,
                    original_hash,
                    update_hash: block_id.hash,
                });
            } else {
                return Ok(ChangeSet::default());
            }
        }

        let mut changeset = ChangeSet::default();
        changeset.insert(block_id.height, Some(block_id.hash));
        self.apply_changeset(&changeset);
        Ok(changeset)
    }

    /// Reindex the heights in the chain from (and including) `from` height
    fn reindex(&mut self, from: u32) {
        let _ = self.index.split_off(&from);
        for cp in self.iter_checkpoints() {
            if cp.height() < from {
                break;
            }
            self.index.insert(cp.height(), cp.hash());
        }
    }

    /// Derives an initial [`ChangeSet`], meaning that it can be applied to an empty chain to
    /// recover the current chain.
    pub fn initial_changeset(&self) -> ChangeSet {
        self.index.iter().map(|(k, v)| (*k, Some(*v))).collect()
    }

    /// Iterate over checkpoints in decending height order.
    pub fn iter_checkpoints(&self) -> CheckPointIter {
        CheckPointIter {
            current: self.tip.as_ref().map(|tip| tip.0.clone()),
        }
    }

    /// Get a reference to the internal index mapping the height to block hash.
    pub fn heights(&self) -> &BTreeMap<u32, BlockHash> {
        &self.index
    }

    fn _check_index_is_consistent_with_tip(&self) -> bool {
        let tip_history = self
            .tip
            .iter()
            .flat_map(CheckPoint::iter)
            .map(|cp| (cp.height(), cp.hash()))
            .collect::<BTreeMap<_, _>>();
        self.index == tip_history
    }

    fn _check_changeset_is_applied(&self, changeset: &ChangeSet) -> bool {
        for (height, exp_hash) in changeset {
            if self.index.get(height) != exp_hash.as_ref() {
                return false;
            }
        }
        true
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
    pub try_include_height: u32,
}

impl core::fmt::Display for CannotConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "introduced chain cannot connect with the original chain, try include height {}",
            self.try_include_height,
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CannotConnectError {}

fn merge_chains(
    original_tip: CheckPoint,
    update_tip: CheckPoint,
    introduce_older_blocks: bool,
) -> Result<ChangeSet, CannotConnectError> {
    let mut changeset = ChangeSet::default();
    let mut orig = original_tip.into_iter();
    let mut update = update_tip.into_iter();
    let mut curr_orig = None;
    let mut curr_update = None;
    let mut prev_orig: Option<CheckPoint> = None;
    let mut prev_update: Option<CheckPoint> = None;
    let mut point_of_agreement_found = false;
    let mut prev_orig_was_invalidated = false;
    let mut potentially_invalidated_heights = vec![];

    // To find the difference between the new chain and the original we iterate over both of them
    // from the tip backwards in tandem. We always dealing with the highest one from either chain
    // first and move to the next highest. The crucial logic is applied when they have blocks at the
    // same height.
    loop {
        if curr_orig.is_none() {
            curr_orig = orig.next();
        }
        if curr_update.is_none() {
            curr_update = update.next();
        }

        match (curr_orig.as_ref(), curr_update.as_ref()) {
            // Update block that doesn't exist in the original chain
            (o, Some(u)) if Some(u.height()) > o.map(|o| o.height()) => {
                changeset.insert(u.height(), Some(u.hash()));
                prev_update = curr_update.take();
            }
            // Original block that isn't in the update
            (Some(o), u) if Some(o.height()) > u.map(|u| u.height()) => {
                // this block might be gone if an earlier block gets invalidated
                potentially_invalidated_heights.push(o.height());
                prev_orig_was_invalidated = false;
                prev_orig = curr_orig.take();

                // OPTIMIZATION: we have run out of update blocks so we don't need to continue
                // iterating becuase there's no possibility of adding anything to changeset.
                if u.is_none() {
                    break;
                }
            }
            (Some(o), Some(u)) => {
                if o.hash() == u.hash() {
                    // We have found our point of agreement 🎉 -- we require that the previous (i.e.
                    // higher because we are iterating backwards) block in the original chain was
                    // invalidated (if it exists). This ensures that there is an unambigious point of
                    // connection to the original chain from the update chain (i.e. we know the
                    // precisely which original blocks are invalid).
                    if !prev_orig_was_invalidated && !point_of_agreement_found {
                        if let (Some(prev_orig), Some(_prev_update)) = (&prev_orig, &prev_update) {
                            return Err(CannotConnectError {
                                try_include_height: prev_orig.height(),
                            });
                        }
                    }
                    point_of_agreement_found = true;
                    prev_orig_was_invalidated = false;
                    // OPTIMIZATION 1 -- If we know that older blocks cannot be introduced without
                    // invalidation, we can break after finding the point of agreement.
                    // OPTIMIZATION 2 -- if we have the same underlying pointer at this point, we
                    // can guarantee that no older blocks are introduced.
                    if !introduce_older_blocks || Arc::as_ptr(&o.0) == Arc::as_ptr(&u.0) {
                        return Ok(changeset);
                    }
                } else {
                    // We have an invalidation height so we set the height to the updated hash and
                    // also purge all the original chain block hashes above this block.
                    changeset.insert(u.height(), Some(u.hash()));
                    for invalidated_height in potentially_invalidated_heights.drain(..) {
                        changeset.insert(invalidated_height, None);
                    }
                    prev_orig_was_invalidated = true;
                }
                prev_update = curr_update.take();
                prev_orig = curr_orig.take();
            }
            (None, None) => {
                break;
            }
            _ => {
                unreachable!("compiler cannot tell that everything has been covered")
            }
        }
    }

    // When we don't have a point of agreement you can imagine it is implicitly the
    // genesis block so we need to do the final connectivity check which in this case
    // just means making sure the entire original chain was invalidated.
    if !prev_orig_was_invalidated && !point_of_agreement_found {
        if let Some(prev_orig) = prev_orig {
            return Err(CannotConnectError {
                try_include_height: prev_orig.height(),
            });
        }
    }

    Ok(changeset)
}
