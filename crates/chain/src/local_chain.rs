//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;
use core::ops::RangeBounds;

use crate::collections::BTreeMap;
use crate::{BlockId, ChainOracle};
use alloc::sync::Arc;
use bitcoin::block::Header;
use bitcoin::BlockHash;

/// The [`ChangeSet`] represents changes to [`LocalChain`].
///
/// The key represents the block height, and the value either represents added a new [`CheckPoint`]
/// (if [`Some`]), or removing a [`CheckPoint`] (if [`None`]).
pub type ChangeSet<B = BlockHash> = BTreeMap<u32, Option<B>>;

/// A [`LocalChain`] checkpoint is used to find the agreement point between two chains and as a
/// transaction anchor.
///
/// Each checkpoint contains the height and hash of a block ([`BlockId`]).
///
/// Internally, checkpoints are nodes of a reference-counted linked-list. This allows the caller to
/// cheaply clone a [`CheckPoint`] without copying the whole list and to view the entire chain
/// without holding a lock on [`LocalChain`].
#[derive(Debug, Clone)]
pub struct CheckPoint<B = BlockHash>(Arc<CPInner<B>>);

/// The internal contents of [`CheckPoint`].
#[derive(Debug, Clone)]
struct CPInner<B = BlockHash> {
    /// Block height
    height: u32,
    /// Block data
    block_data: B,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner>>,
}

impl PartialEq for CheckPoint {
    fn eq(&self, other: &Self) -> bool {
        let self_cps = self.iter().map(|cp| cp.block_id());
        let other_cps = other.iter().map(|cp| cp.block_id());
        self_cps.eq(other_cps)
    }
}

impl CheckPoint {
    /// Construct a new base block at the front of a linked list.
    pub fn new(block: BlockId) -> Self {
        let height = block.height;
        let block = block.hash;
        Self(Arc::new(CPInner {
            height,
            block_data: block,
            prev: None,
        }))
    }

    /// Construct a checkpoint from a list of [`BlockId`]s in ascending height order.
    ///
    /// # Errors
    ///
    /// This method will error if any of the follow occurs:
    ///
    /// - The `blocks` iterator is empty, in which case, the error will be `None`.
    /// - The `blocks` iterator is not in ascending height order.
    /// - The `blocks` iterator contains multiple [`BlockId`]s of the same height.
    ///
    /// The error type is the last successful checkpoint constructed (if any).
    pub fn from_block_ids(
        block_ids: impl IntoIterator<Item = BlockId>,
    ) -> Result<Self, Option<Self>> {
        let mut blocks = block_ids.into_iter();
        let mut acc = CheckPoint::new(blocks.next().ok_or(None)?);
        for id in blocks {
            acc = acc.push(id).map_err(Some)?;
        }
        Ok(acc)
    }

    /// Construct a checkpoint from the given `header` and block `height`.
    ///
    /// If `header` is of the genesis block, the checkpoint won't have a [`prev`] node. Otherwise,
    /// we return a checkpoint linked with the previous block.
    ///
    /// [`prev`]: CheckPoint::prev
    pub fn from_header(header: &bitcoin::block::Header, height: u32) -> Self {
        let hash = header.block_hash();
        let this_block_id = BlockId { height, hash };

        let prev_height = match height.checked_sub(1) {
            Some(h) => h,
            None => return Self::new(this_block_id),
        };

        let prev_block_id = BlockId {
            height: prev_height,
            hash: header.prev_blockhash,
        };

        CheckPoint::new(prev_block_id)
            .push(this_block_id)
            .expect("must construct checkpoint")
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the one you
    /// are pushing on to.
    pub fn push(self, block: BlockId) -> Result<Self, Self> {
        if self.height() < block.height {
            Ok(Self(Arc::new(CPInner {
                height: block.height,
                block_data: block.hash,
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
    pub fn extend(self, blocks: impl IntoIterator<Item = BlockId>) -> Result<Self, Self> {
        let mut curr = self.clone();
        for block in blocks {
            curr = curr.push(block).map_err(|_| self.clone())?;
        }
        Ok(curr)
    }

    /// Get the [`BlockId`] of the checkpoint.
    pub fn block_id(&self) -> BlockId {
        BlockId {
            height: self.0.height,
            hash: self.0.block_data,
        }
    }

    /// Get the height of the checkpoint.
    pub fn height(&self) -> u32 {
        self.0.height
    }

    /// Get the block hash of the checkpoint.
    pub fn hash(&self) -> BlockHash {
        self.0.block_data
    }

    /// Get the previous checkpoint in the chain
    pub fn prev(&self) -> Option<CheckPoint> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter {
        self.clone().into_iter()
    }

    /// Get checkpoint at `height`.
    ///
    /// Returns `None` if checkpoint at `height` does not exist`.
    pub fn get(&self, height: u32) -> Option<Self> {
        self.range(height..=height).next()
    }

    /// Iterate checkpoints over a height range.
    ///
    /// Note that we always iterate checkpoints in reverse height order (iteration starts at tip
    /// height).
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint>
    where
        R: RangeBounds<u32>,
    {
        let start_bound = range.start_bound().cloned();
        let end_bound = range.end_bound().cloned();
        self.iter()
            .skip_while(move |cp| match end_bound {
                core::ops::Bound::Included(inc_bound) => cp.height() > inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp.height() >= exc_bound,
                core::ops::Bound::Unbounded => false,
            })
            .take_while(move |cp| match start_bound {
                core::ops::Bound::Included(inc_bound) => cp.height() >= inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp.height() > exc_bound,
                core::ops::Bound::Unbounded => true,
            })
    }

    /// Inserts `block_id` at its height within the chain.
    ///
    /// The effect of `insert` depends on whether a height already exists. If it doesn't the
    /// `block_id` we inserted and all pre-existing blocks higher than it will be re-inserted after
    /// it. If the height already existed and has a conflicting block hash then it will be purged
    /// along with all block followin it. The returned chain will have a tip of the `block_id`
    /// passed in. Of course, if the `block_id` was already present then this just returns `self`.
    #[must_use]
    pub fn insert(self, block_id: BlockId) -> Self {
        assert_ne!(block_id.height, 0, "cannot insert the genesis block");

        let mut cp = self.clone();
        let mut tail = vec![];
        let base = loop {
            if cp.height() == block_id.height {
                if cp.hash() == block_id.hash {
                    return self;
                }
                // if we have a conflict we just return the inserted block because the tail is by
                // implication invalid.
                tail = vec![];
                break cp.prev().expect("can't be called on genesis block");
            }

            if cp.height() < block_id.height {
                break cp;
            }

            tail.push(cp.block_id());
            cp = cp.prev().expect("will break before genesis block");
        };

        base.extend(core::iter::once(block_id).chain(tail.into_iter().rev()))
            .expect("tail is in order")
    }

    /// Apply `changeset` to the checkpoint.
    fn apply_changeset(mut self, changeset: &ChangeSet) -> Result<CheckPoint, MissingGenesisError> {
        if let Some(start_height) = changeset.keys().next().cloned() {
            // changes after point of agreement
            let mut extension = BTreeMap::default();
            // point of agreement
            let mut base: Option<CheckPoint> = None;

            for cp in self.iter() {
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
                Some(base) => base
                    .extend(extension.into_iter().map(BlockId::from))
                    .expect("extension is strictly greater than base"),
                None => LocalChain::from_blocks(extension)?.tip(),
            };
            self = new_tip;
        }

        Ok(self)
    }
}

/// Iterates over checkpoints backwards.
pub struct CheckPointIter {
    current: Option<Arc<CPInner>>,
}

impl Iterator for CheckPointIter {
    type Item = CheckPoint;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.clone()?;
        self.current.clone_from(&current.prev);
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

/// This is a local implementation of [`ChainOracle`].
#[derive(Debug, Clone)]
pub struct LocalChain<B = BlockHash> {
    tip: CheckPoint<B>,
}

impl PartialEq for LocalChain {
    fn eq(&self, other: &Self) -> bool {
        self.tip == other.tip
    }
}

impl ChainOracle for LocalChain {
    type Error = Infallible;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        chain_tip: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        let chain_tip_cp = match self.tip.get(chain_tip.height) {
            // we can only determine whether `block` is in chain of `chain_tip` if `chain_tip` can
            // be identified in chain
            Some(cp) if cp.hash() == chain_tip.hash => cp,
            _ => return Ok(None),
        };
        match chain_tip_cp.get(block.height) {
            Some(cp) => Ok(Some(cp.hash() == block.hash)),
            None => Ok(None),
        }
    }

    fn get_chain_tip(&self) -> Result<BlockId, Self::Error> {
        Ok(self.tip.block_id())
    }
}

impl LocalChain {
    /// Get the genesis hash.
    pub fn genesis_hash(&self) -> BlockHash {
        self.tip.get(0).expect("genesis must exist").hash()
    }

    /// Construct [`LocalChain`] from genesis `hash`.
    #[must_use]
    pub fn from_genesis_hash(hash: BlockHash) -> (Self, ChangeSet) {
        let height = 0;
        let chain = Self {
            tip: CheckPoint::new(BlockId { height, hash }),
        };
        let changeset = chain.initial_changeset();
        (chain, changeset)
    }

    /// Construct a [`LocalChain`] from an initial `changeset`.
    pub fn from_changeset(changeset: ChangeSet) -> Result<Self, MissingGenesisError> {
        let genesis_entry = changeset.get(&0).copied().flatten();
        let genesis_hash = match genesis_entry {
            Some(hash) => hash,
            None => return Err(MissingGenesisError),
        };

        let (mut chain, _) = Self::from_genesis_hash(genesis_hash);
        chain.apply_changeset(&changeset)?;

        debug_assert!(chain._check_changeset_is_applied(&changeset));

        Ok(chain)
    }

    /// Construct a [`LocalChain`] from a given `checkpoint` tip.
    pub fn from_tip(tip: CheckPoint) -> Result<Self, MissingGenesisError> {
        let genesis_cp = tip.iter().last().expect("must have at least one element");
        if genesis_cp.height() != 0 {
            return Err(MissingGenesisError);
        }
        Ok(Self { tip })
    }

    /// Constructs a [`LocalChain`] from a [`BTreeMap`] of height to [`BlockHash`].
    ///
    /// The [`BTreeMap`] enforces the height order. However, the caller must ensure the blocks are
    /// all of the same chain.
    pub fn from_blocks(blocks: BTreeMap<u32, BlockHash>) -> Result<Self, MissingGenesisError> {
        if !blocks.contains_key(&0) {
            return Err(MissingGenesisError);
        }

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

        Ok(Self {
            tip: tip.expect("already checked to have genesis"),
        })
    }

    /// Get the highest checkpoint.
    pub fn tip(&self) -> CheckPoint {
        self.tip.clone()
    }

    /// Applies the given `update` to the chain.
    ///
    /// The method returns [`ChangeSet`] on success. This represents the changes applied to `self`.
    ///
    /// There must be no ambiguity about which of the existing chain's blocks are still valid and
    /// which are now invalid. That is, the new chain must implicitly connect to a definite block in
    /// the existing chain and invalidate the block after it (if it exists) by including a block at
    /// the same height but with a different hash to explicitly exclude it as a connection point.
    ///
    /// # Errors
    ///
    /// An error will occur if the update does not correctly connect with `self`.
    ///
    /// [module-level documentation]: crate::local_chain
    pub fn apply_update(&mut self, update: CheckPoint) -> Result<ChangeSet, CannotConnectError> {
        let (new_tip, changeset) = merge_chains(self.tip.clone(), update)?;
        self.tip = new_tip;
        self._check_changeset_is_applied(&changeset);
        Ok(changeset)
    }

    /// Update the chain with a given [`Header`] at `height` which you claim is connected to a existing block in the chain.
    ///
    /// This is useful when you have a block header that you want to record as part of the chain but
    /// don't necessarily know that the `prev_blockhash` is in the chain.
    ///
    /// This will usually insert two new [`BlockId`]s into the chain: the header's block and the
    /// header's `prev_blockhash` block. `connected_to` must already be in the chain but is allowed
    /// to be `prev_blockhash` (in which case only one new block id will be inserted).
    /// To be successful, `connected_to` must be chosen carefully so that `LocalChain`'s [update
    /// rules][`apply_update`] are satisfied.
    ///
    /// # Errors
    ///
    /// [`ApplyHeaderError::InconsistentBlocks`] occurs if the `connected_to` block and the
    /// [`Header`] is inconsistent. For example, if the `connected_to` block is the same height as
    /// `header` or `prev_blockhash`, but has a different block hash. Or if the `connected_to`
    /// height is greater than the header's `height`.
    ///
    /// [`ApplyHeaderError::CannotConnect`] occurs if the internal call to [`apply_update`] fails.
    ///
    /// [`apply_update`]: Self::apply_update
    pub fn apply_header_connected_to(
        &mut self,
        header: &Header,
        height: u32,
        connected_to: BlockId,
    ) -> Result<ChangeSet, ApplyHeaderError> {
        let this = BlockId {
            height,
            hash: header.block_hash(),
        };
        let prev = height.checked_sub(1).map(|prev_height| BlockId {
            height: prev_height,
            hash: header.prev_blockhash,
        });
        let conn = match connected_to {
            // `connected_to` can be ignored if same as `this` or `prev` (duplicate)
            conn if conn == this || Some(conn) == prev => None,
            // this occurs if:
            // - `connected_to` height is the same as `prev`, but different hash
            // - `connected_to` height is the same as `this`, but different hash
            // - `connected_to` height is greater than `this` (this is not allowed)
            conn if conn.height >= height.saturating_sub(1) => {
                return Err(ApplyHeaderError::InconsistentBlocks)
            }
            conn => Some(conn),
        };

        let update = CheckPoint::from_block_ids([conn, prev, Some(this)].into_iter().flatten())
            .expect("block ids must be in order");

        self.apply_update(update)
            .map_err(ApplyHeaderError::CannotConnect)
    }

    /// Update the chain with a given [`Header`] connecting it with the previous block.
    ///
    /// This is a convenience method to call [`apply_header_connected_to`] with the `connected_to`
    /// parameter being `height-1:prev_blockhash`. If there is no previous block (i.e. genesis), we
    /// use the current block as `connected_to`.
    ///
    /// [`apply_header_connected_to`]: LocalChain::apply_header_connected_to
    pub fn apply_header(
        &mut self,
        header: &Header,
        height: u32,
    ) -> Result<ChangeSet, CannotConnectError> {
        let connected_to = match height.checked_sub(1) {
            Some(prev_height) => BlockId {
                height: prev_height,
                hash: header.prev_blockhash,
            },
            None => BlockId {
                height,
                hash: header.block_hash(),
            },
        };
        self.apply_header_connected_to(header, height, connected_to)
            .map_err(|err| match err {
                ApplyHeaderError::InconsistentBlocks => {
                    unreachable!("connected_to is derived from the block so is always consistent")
                }
                ApplyHeaderError::CannotConnect(err) => err,
            })
    }

    /// Apply the given `changeset`.
    pub fn apply_changeset(&mut self, changeset: &ChangeSet) -> Result<(), MissingGenesisError> {
        let old_tip = self.tip.clone();
        let new_tip = old_tip.apply_changeset(changeset)?;
        self.tip = new_tip;
        debug_assert!(self._check_changeset_is_applied(changeset));
        Ok(())
    }

    /// Insert a [`BlockId`].
    ///
    /// # Errors
    ///
    /// Replacing the block hash of an existing checkpoint will result in an error.
    pub fn insert_block(&mut self, block_id: BlockId) -> Result<ChangeSet, AlterCheckPointError> {
        if let Some(original_cp) = self.tip.get(block_id.height) {
            let original_hash = original_cp.hash();
            if original_hash != block_id.hash {
                return Err(AlterCheckPointError {
                    height: block_id.height,
                    original_hash,
                    update_hash: Some(block_id.hash),
                });
            }
            return Ok(ChangeSet::default());
        }

        let mut changeset = ChangeSet::default();
        changeset.insert(block_id.height, Some(block_id.hash));
        self.apply_changeset(&changeset)
            .map_err(|_| AlterCheckPointError {
                height: 0,
                original_hash: self.genesis_hash(),
                update_hash: changeset.get(&0).cloned().flatten(),
            })?;
        Ok(changeset)
    }

    /// Removes blocks from (and inclusive of) the given `block_id`.
    ///
    /// This will remove blocks with a height equal or greater than `block_id`, but only if
    /// `block_id` exists in the chain.
    ///
    /// # Errors
    ///
    /// This will fail with [`MissingGenesisError`] if the caller attempts to disconnect from the
    /// genesis block.
    pub fn disconnect_from(&mut self, block_id: BlockId) -> Result<ChangeSet, MissingGenesisError> {
        let mut remove_from = Option::<CheckPoint>::None;
        let mut changeset = ChangeSet::default();
        for cp in self.tip().iter() {
            let cp_id = cp.block_id();
            if cp_id.height < block_id.height {
                break;
            }
            changeset.insert(cp_id.height, None);
            if cp_id == block_id {
                remove_from = Some(cp);
            }
        }
        self.tip = match remove_from.map(|cp| cp.prev()) {
            // The checkpoint below the earliest checkpoint to remove will be the new tip.
            Some(Some(new_tip)) => new_tip,
            // If there is no checkpoint below the earliest checkpoint to remove, it means the
            // "earliest checkpoint to remove" is the genesis block. We disallow removing the
            // genesis block.
            Some(None) => return Err(MissingGenesisError),
            // If there is nothing to remove, we return an empty changeset.
            None => return Ok(ChangeSet::default()),
        };
        Ok(changeset)
    }

    /// Derives an initial [`ChangeSet`], meaning that it can be applied to an empty chain to
    /// recover the current chain.
    pub fn initial_changeset(&self) -> ChangeSet {
        self.tip
            .iter()
            .map(|cp| {
                let block_id = cp.block_id();
                (block_id.height, Some(block_id.hash))
            })
            .collect()
    }

    /// Iterate over checkpoints in descending height order.
    pub fn iter_checkpoints(&self) -> CheckPointIter {
        CheckPointIter {
            current: Some(self.tip.0.clone()),
        }
    }

    fn _check_changeset_is_applied(&self, changeset: &ChangeSet) -> bool {
        let mut curr_cp = self.tip.clone();
        for (height, exp_hash) in changeset.iter().rev() {
            match curr_cp.get(*height) {
                Some(query_cp) => {
                    if query_cp.height() != *height || Some(query_cp.hash()) != *exp_hash {
                        return false;
                    }
                    curr_cp = query_cp;
                }
                None => {
                    if exp_hash.is_some() {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Get checkpoint at given `height` (if it exists).
    ///
    /// This is a shorthand for calling [`CheckPoint::get`] on the [`tip`].
    ///
    /// [`tip`]: LocalChain::tip
    pub fn get(&self, height: u32) -> Option<CheckPoint> {
        self.tip.get(height)
    }

    /// Iterate checkpoints over a height range.
    ///
    /// Note that we always iterate checkpoints in reverse height order (iteration starts at tip
    /// height).
    ///
    /// This is a shorthand for calling [`CheckPoint::range`] on the [`tip`].
    ///
    /// [`tip`]: LocalChain::tip
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint>
    where
        R: RangeBounds<u32>,
    {
        self.tip.range(range)
    }
}

/// An error which occurs when a [`LocalChain`] is constructed without a genesis checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct MissingGenesisError;

impl core::fmt::Display for MissingGenesisError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "cannot construct `LocalChain` without a genesis checkpoint"
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MissingGenesisError {}

/// Represents a failure when trying to insert/remove a checkpoint to/from [`LocalChain`].
#[derive(Clone, Debug, PartialEq)]
pub struct AlterCheckPointError {
    /// The checkpoint's height.
    pub height: u32,
    /// The original checkpoint's block hash which cannot be replaced/removed.
    pub original_hash: BlockHash,
    /// The attempted update to the `original_block` hash.
    pub update_hash: Option<BlockHash>,
}

impl core::fmt::Display for AlterCheckPointError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.update_hash {
            Some(update_hash) => write!(
                f,
                "failed to insert block at height {}: original={} update={}",
                self.height, self.original_hash, update_hash
            ),
            None => write!(
                f,
                "failed to remove block at height {}: original={}",
                self.height, self.original_hash
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AlterCheckPointError {}

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

/// The error type for [`LocalChain::apply_header_connected_to`].
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyHeaderError {
    /// Occurs when `connected_to` block conflicts with either the current block or previous block.
    InconsistentBlocks,
    /// Occurs when the update cannot connect with the original chain.
    CannotConnect(CannotConnectError),
}

impl core::fmt::Display for ApplyHeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ApplyHeaderError::InconsistentBlocks => write!(
                f,
                "the `connected_to` block conflicts with either the current or previous block"
            ),
            ApplyHeaderError::CannotConnect(err) => core::fmt::Display::fmt(err, f),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ApplyHeaderError {}

/// Applies `update_tip` onto `original_tip`.
///
/// On success, a tuple is returned `(changeset, can_replace)`. If `can_replace` is true, then the
/// `update_tip` can replace the `original_tip`.
fn merge_chains(
    original_tip: CheckPoint,
    update_tip: CheckPoint,
) -> Result<(CheckPoint, ChangeSet), CannotConnectError> {
    let mut changeset = ChangeSet::default();
    let mut orig = original_tip.iter();
    let mut update = update_tip.iter();
    let mut curr_orig = None;
    let mut curr_update = None;
    let mut prev_orig: Option<CheckPoint> = None;
    let mut prev_update: Option<CheckPoint> = None;
    let mut point_of_agreement_found = false;
    let mut prev_orig_was_invalidated = false;
    let mut potentially_invalidated_heights = vec![];

    // If we can, we want to return the update tip as the new tip because this allows checkpoints
    // in multiple locations to keep the same `Arc` pointers when they are being updated from each
    // other using this function. We can do this as long as long as the update contains every
    // block's height of the original chain.
    let mut is_update_height_superset_of_original = true;

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

                is_update_height_superset_of_original = false;

                // OPTIMIZATION: we have run out of update blocks so we don't need to continue
                // iterating because there's no possibility of adding anything to changeset.
                if u.is_none() {
                    break;
                }
            }
            (Some(o), Some(u)) => {
                if o.hash() == u.hash() {
                    // We have found our point of agreement 🎉 -- we require that the previous (i.e.
                    // higher because we are iterating backwards) block in the original chain was
                    // invalidated (if it exists). This ensures that there is an unambiguous point of
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
                    // OPTIMIZATION 2 -- if we have the same underlying pointer at this point, we
                    // can guarantee that no older blocks are introduced.
                    if Arc::as_ptr(&o.0) == Arc::as_ptr(&u.0) {
                        if is_update_height_superset_of_original {
                            return Ok((update_tip, changeset));
                        } else {
                            let new_tip =
                                original_tip.apply_changeset(&changeset).map_err(|_| {
                                    CannotConnectError {
                                        try_include_height: 0,
                                    }
                                })?;
                            return Ok((new_tip, changeset));
                        }
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

    let new_tip = original_tip
        .apply_changeset(&changeset)
        .map_err(|_| CannotConnectError {
            try_include_height: 0,
        })?;
    Ok((new_tip, changeset))
}
