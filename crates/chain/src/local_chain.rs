//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;

use crate::collections::BTreeMap;
use crate::{BlockId, ChainOracle};
use alloc::sync::Arc;
use bitcoin::block::Header;
use bitcoin::BlockHash;

/// The [`ChangeSet`] represents changes to [`LocalChain`].
///
/// The key represents the block height, and the value either represents added a new [`CheckPoint`]
/// (if [`Some`]), or removing a [`CheckPoint`] (if [`None`]).
pub type ChangeSet = BTreeMap<u32, Option<BlockHash>>;

/// A [`LocalChain`] checkpoint is used to find the agreement point between two chains and as a
/// transaction anchor.
///
/// Each checkpoint contains the height and hash of a block ([`BlockId`]).
///
/// Internally, checkpoints are nodes of a reference-counted linked-list. This allows the caller to
/// cheaply clone a [`CheckPoint`] without copying the whole list and to view the entire chain
/// without holding a lock on [`LocalChain`].
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

    /// Convenience method to convert the [`CheckPoint`] into an [`Update`].
    ///
    /// For more information, refer to [`Update`].
    pub fn into_update(self, introduce_older_blocks: bool) -> Update {
        Update {
            tip: self,
            introduce_older_blocks,
        }
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
    pub fn extend(self, blocks: impl IntoIterator<Item = BlockId>) -> Result<Self, Self> {
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

/// Iterates over checkpoints backwards.
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

/// Used to update [`LocalChain`].
///
/// This is used as input for [`LocalChain::apply_update`]. It contains the update's chain `tip` and
/// a flag `introduce_older_blocks` which signals whether this update intends to introduce missing
/// blocks to the original chain.
///
/// Block-by-block syncing mechanisms would typically create updates that builds upon the previous
/// tip. In this case, `introduce_older_blocks` would be `false`.
///
/// Script-pubkey based syncing mechanisms may not introduce transactions in a chronological order
/// so some updates require introducing older blocks (to anchor older transactions). For
/// script-pubkey based syncing, `introduce_older_blocks` would typically be `true`.
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
#[derive(Debug, Clone)]
pub struct LocalChain {
    tip: CheckPoint,
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

    fn get_chain_tip(&self) -> Result<BlockId, Self::Error> {
        Ok(self.tip.block_id())
    }
}

impl LocalChain {
    /// Get the genesis hash.
    pub fn genesis_hash(&self) -> BlockHash {
        self.index.get(&0).copied().expect("must have genesis hash")
    }

    /// Construct [`LocalChain`] from genesis `hash`.
    #[must_use]
    pub fn from_genesis_hash(hash: BlockHash) -> (Self, ChangeSet) {
        let height = 0;
        let chain = Self {
            tip: CheckPoint::new(BlockId { height, hash }),
            index: core::iter::once((height, hash)).collect(),
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

        debug_assert!(chain._check_index_is_consistent_with_tip());
        debug_assert!(chain._check_changeset_is_applied(&changeset));

        Ok(chain)
    }

    /// Construct a [`LocalChain`] from a given `checkpoint` tip.
    pub fn from_tip(tip: CheckPoint) -> Result<Self, MissingGenesisError> {
        let mut chain = Self {
            tip,
            index: BTreeMap::new(),
        };
        chain.reindex(0);

        if chain.index.get(&0).copied().is_none() {
            return Err(MissingGenesisError);
        }

        debug_assert!(chain._check_index_is_consistent_with_tip());
        Ok(chain)
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

        let chain = Self {
            index: blocks,
            tip: tip.expect("already checked to have genesis"),
        };

        debug_assert!(chain._check_index_is_consistent_with_tip());
        Ok(chain)
    }

    /// Get the highest checkpoint.
    pub fn tip(&self) -> CheckPoint {
        self.tip.clone()
    }

    /// Applies the given `update` to the chain.
    ///
    /// The method returns [`ChangeSet`] on success. This represents the applied changes to `self`.
    ///
    /// There must be no ambiguity about which of the existing chain's blocks are still valid and
    /// which are now invalid. That is, the new chain must implicitly connect to a definite block in
    /// the existing chain and invalidate the block after it (if it exists) by including a block at
    /// the same height but with a different hash to explicitly exclude it as a connection point.
    ///
    /// Additionally, an empty chain can be updated with any chain, and a chain with a single block
    /// can have it's block invalidated by an update chain with a block at the same height but
    /// different hash.
    ///
    /// # Errors
    ///
    /// An error will occur if the update does not correctly connect with `self`.
    ///
    /// Refer to [`Update`] for more about the update struct.
    ///
    /// [module-level documentation]: crate::local_chain
    pub fn apply_update(&mut self, update: Update) -> Result<ChangeSet, CannotConnectError> {
        let changeset = merge_chains(
            self.tip.clone(),
            update.tip.clone(),
            update.introduce_older_blocks,
        )?;
        // `._check_index_is_consistent_with_tip` and `._check_changeset_is_applied` is called in
        // `.apply_changeset`
        self.apply_changeset(&changeset)
            .map_err(|_| CannotConnectError {
                try_include_height: 0,
            })?;
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

        let update = Update {
            tip: CheckPoint::from_block_ids([conn, prev, Some(this)].into_iter().flatten())
                .expect("block ids must be in order"),
            introduce_older_blocks: false,
        };

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
        if let Some(start_height) = changeset.keys().next().cloned() {
            // changes after point of agreement
            let mut extension = BTreeMap::default();
            // point of agreement
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
                Some(base) => base
                    .extend(extension.into_iter().map(BlockId::from))
                    .expect("extension is strictly greater than base"),
                None => LocalChain::from_blocks(extension)?.tip(),
            };
            self.tip = new_tip;
            self.reindex(start_height);

            debug_assert!(self._check_index_is_consistent_with_tip());
            debug_assert!(self._check_changeset_is_applied(changeset));
        }

        Ok(())
    }

    /// Insert a [`BlockId`].
    ///
    /// # Errors
    ///
    /// Replacing the block hash of an existing checkpoint will result in an error.
    pub fn insert_block(&mut self, block_id: BlockId) -> Result<ChangeSet, AlterCheckPointError> {
        if let Some(&original_hash) = self.index.get(&block_id.height) {
            if original_hash != block_id.hash {
                return Err(AlterCheckPointError {
                    height: block_id.height,
                    original_hash,
                    update_hash: Some(block_id.hash),
                });
            } else {
                return Ok(ChangeSet::default());
            }
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
        if self.index.get(&block_id.height) != Some(&block_id.hash) {
            return Ok(ChangeSet::default());
        }

        let changeset = self
            .index
            .range(block_id.height..)
            .map(|(&height, _)| (height, None))
            .collect::<ChangeSet>();
        self.apply_changeset(&changeset).map(|_| changeset)
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

    /// Iterate over checkpoints in descending height order.
    pub fn iter_checkpoints(&self) -> CheckPointIter {
        CheckPointIter {
            current: Some(self.tip.0.clone()),
        }
    }

    /// Get a reference to the internal index mapping the height to block hash.
    pub fn blocks(&self) -> &BTreeMap<u32, BlockHash> {
        &self.index
    }

    fn _check_index_is_consistent_with_tip(&self) -> bool {
        let tip_history = self
            .tip
            .iter()
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

/// An error which occurs when a [`LocalChain`] is constructed without a genesis checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
