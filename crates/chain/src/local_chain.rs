//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;
use core::fmt;
use core::ops::RangeBounds;

use crate::canonical_task::{CanonicalizationRequest, CanonicalizationResponse};
use crate::collections::BTreeMap;
use crate::{Anchor, BlockId, ChainOracle, Merge};
use bdk_core::ToBlockHash;
pub use bdk_core::{CheckPoint, CheckPointIter};
use bitcoin::block::Header;
use bitcoin::BlockHash;

/// Apply `changeset` to the checkpoint.
fn apply_changeset_to_checkpoint<D>(
    mut init_cp: CheckPoint<D>,
    changeset: &ChangeSet<D>,
) -> Result<CheckPoint<D>, MissingGenesisError>
where
    D: ToBlockHash + fmt::Debug + Copy,
{
    if let Some(start_height) = changeset.blocks.keys().next().cloned() {
        // changes after point of agreement
        let mut extension = BTreeMap::default();
        // point of agreement
        let mut base: Option<CheckPoint<D>> = None;

        for cp in init_cp.iter() {
            if cp.height() >= start_height {
                extension.insert(cp.height(), cp.data());
            } else {
                base = Some(cp);
                break;
            }
        }

        for (&height, &data) in &changeset.blocks {
            match data {
                Some(data) => {
                    extension.insert(height, data);
                }
                None => {
                    extension.remove(&height);
                }
            };
        }

        let new_tip = match base {
            Some(base) => base
                .extend(extension)
                .expect("extension is strictly greater than base"),
            None => LocalChain::from_blocks(extension)?.tip(),
        };
        init_cp = new_tip;
    }

    Ok(init_cp)
}

/// This is a local implementation of [`ChainOracle`].
#[derive(Debug, Clone)]
pub struct LocalChain<D = BlockHash> {
    tip: CheckPoint<D>,
}

impl<D> PartialEq for LocalChain<D> {
    fn eq(&self, other: &Self) -> bool {
        self.tip == other.tip
    }
}

impl<D> ChainOracle for LocalChain<D> {
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

// Methods for `LocalChain<BlockHash>`
impl LocalChain<BlockHash> {
    /// Handle a canonicalization request.
    ///
    /// This method processes requests from [`CanonicalizationTask`] to check if blocks
    /// are in the chain.
    ///
    /// [`CanonicalizationTask`]: crate::canonical_task::CanonicalizationTask
    pub fn handle_canonicalization_request<A: Anchor>(
        &self,
        request: &CanonicalizationRequest<A>,
    ) -> Result<CanonicalizationResponse<A>, Infallible> {
        // Check each anchor and return the first confirmed one
        for anchor in &request.anchors {
            if self.is_block_in_chain(anchor.anchor_block(), request.chain_tip)? == Some(true) {
                return Ok(Some(anchor.clone()));
            }
        }
        Ok(None)
    }

    /// Update the chain with a given [`Header`] at `height` which you claim is connected to a
    /// existing block in the chain.
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

        let update = CheckPoint::from_blocks(
            [conn, prev, Some(this)]
                .into_iter()
                .flatten()
                .map(|block_id| (block_id.height, block_id.hash)),
        )
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
}

// Methods for any `D`
impl<D> LocalChain<D> {
    /// Get the highest checkpoint.
    pub fn tip(&self) -> CheckPoint<D> {
        self.tip.clone()
    }

    /// Get the genesis hash.
    pub fn genesis_hash(&self) -> BlockHash {
        self.tip.get(0).expect("genesis must exist").block_id().hash
    }

    /// Iterate over checkpoints in descending height order.
    pub fn iter_checkpoints(&self) -> CheckPointIter<D> {
        self.tip.iter()
    }

    /// Get checkpoint at given `height` (if it exists).
    ///
    /// This is a shorthand for calling [`CheckPoint::get`] on the [`tip`].
    ///
    /// [`tip`]: LocalChain::tip
    pub fn get(&self, height: u32) -> Option<CheckPoint<D>> {
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
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint<D>>
    where
        R: RangeBounds<u32>,
    {
        self.tip.range(range)
    }
}

// Methods where `D: ToBlockHash`
impl<D> LocalChain<D>
where
    D: ToBlockHash + fmt::Debug + Copy,
{
    /// Constructs a [`LocalChain`] from genesis data.
    pub fn from_genesis(data: D) -> (Self, ChangeSet<D>) {
        let height = 0;
        let chain = Self {
            tip: CheckPoint::new(height, data),
        };
        let changeset = chain.initial_changeset();

        (chain, changeset)
    }

    /// Constructs a [`LocalChain`] from a [`BTreeMap`] of height and data `D`.
    ///
    /// The [`BTreeMap`] enforces the height order. However, the caller must ensure the blocks are
    /// all of the same chain.
    pub fn from_blocks(blocks: BTreeMap<u32, D>) -> Result<Self, MissingGenesisError> {
        if !blocks.contains_key(&0) {
            return Err(MissingGenesisError);
        }

        Ok(Self {
            tip: CheckPoint::from_blocks(blocks).expect("blocks must be in order"),
        })
    }

    /// Construct a [`LocalChain`] from an initial `changeset`.
    pub fn from_changeset(changeset: ChangeSet<D>) -> Result<Self, MissingGenesisError> {
        let genesis_entry = changeset.blocks.get(&0).copied().flatten();
        let genesis_data = match genesis_entry {
            Some(data) => data,
            None => return Err(MissingGenesisError),
        };

        let (mut chain, _) = Self::from_genesis(genesis_data);
        chain.apply_changeset(&changeset)?;
        debug_assert!(chain._check_changeset_is_applied(&changeset));
        Ok(chain)
    }

    /// Construct a [`LocalChain`] from a given `checkpoint` tip.
    pub fn from_tip(tip: CheckPoint<D>) -> Result<Self, MissingGenesisError> {
        let genesis_cp = tip.iter().last().expect("must have at least one element");
        if genesis_cp.height() != 0 {
            return Err(MissingGenesisError);
        }

        Ok(Self { tip })
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
    pub fn apply_update(
        &mut self,
        update: CheckPoint<D>,
    ) -> Result<ChangeSet<D>, CannotConnectError> {
        let (new_tip, changeset) = merge_chains(self.tip.clone(), update)?;
        self.tip = new_tip;
        debug_assert!(self._check_changeset_is_applied(&changeset));
        Ok(changeset)
    }

    /// Apply the given `changeset`.
    pub fn apply_changeset(&mut self, changeset: &ChangeSet<D>) -> Result<(), MissingGenesisError> {
        let old_tip = self.tip.clone();
        let new_tip = apply_changeset_to_checkpoint(old_tip, changeset)?;
        self.tip = new_tip;
        debug_assert!(self._check_changeset_is_applied(changeset));
        Ok(())
    }

    /// Derives an initial [`ChangeSet`], meaning that it can be applied to an empty chain to
    /// recover the current chain.
    pub fn initial_changeset(&self) -> ChangeSet<D> {
        ChangeSet {
            blocks: self
                .tip
                .iter()
                .map(|cp| (cp.height(), Some(cp.data())))
                .collect(),
        }
    }

    /// Insert block into a [`LocalChain`].
    ///
    /// # Errors
    ///
    /// Replacing the block hash of an existing checkpoint will result in an error.
    pub fn insert_block(
        &mut self,
        height: u32,
        data: D,
    ) -> Result<ChangeSet<D>, AlterCheckPointError> {
        if let Some(original_cp) = self.tip.get(height) {
            let original_hash = original_cp.hash();
            if original_hash != data.to_blockhash() {
                return Err(AlterCheckPointError {
                    height,
                    original_hash,
                    update_hash: Some(data.to_blockhash()),
                });
            }
            return Ok(ChangeSet::default());
        }

        let mut changeset = ChangeSet::<D>::default();
        changeset.blocks.insert(height, Some(data));
        self.apply_changeset(&changeset)
            .map_err(|_| AlterCheckPointError {
                height: 0,
                original_hash: self.genesis_hash(),
                update_hash: changeset
                    .blocks
                    .get(&0)
                    .cloned()
                    .flatten()
                    .map(|d| d.to_blockhash()),
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
    pub fn disconnect_from(
        &mut self,
        block_id: BlockId,
    ) -> Result<ChangeSet<D>, MissingGenesisError> {
        let mut remove_from = Option::<CheckPoint<D>>::None;
        let mut changeset = ChangeSet::default();
        for cp in self.tip().iter() {
            let cp_id = cp.block_id();
            if cp_id.height < block_id.height {
                break;
            }
            changeset.blocks.insert(cp_id.height, None);
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

    fn _check_changeset_is_applied(&self, changeset: &ChangeSet<D>) -> bool {
        let mut cur = self.tip.clone();
        for (&exp_height, exp_data) in changeset.blocks.iter().rev() {
            match cur.get(exp_height) {
                Some(cp) => {
                    if cp.height() != exp_height
                        || Some(cp.hash()) != exp_data.map(|d| d.to_blockhash())
                    {
                        return false;
                    }
                    cur = cp;
                }
                None => {
                    if exp_data.is_some() {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// The [`ChangeSet`] represents changes to [`LocalChain`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ChangeSet<D = BlockHash> {
    /// Changes to the [`LocalChain`] blocks.
    ///
    /// The key represents the block height, and the value either represents added a new
    /// [`CheckPoint`] (if [`Some`]), or removing a [`CheckPoint`] (if [`None`]).
    pub blocks: BTreeMap<u32, Option<D>>,
}

impl<D> Default for ChangeSet<D> {
    fn default() -> Self {
        ChangeSet {
            blocks: BTreeMap::default(),
        }
    }
}

impl<D> Merge for ChangeSet<D> {
    fn merge(&mut self, other: Self) {
        Merge::merge(&mut self.blocks, other.blocks)
    }

    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

impl<D, I: IntoIterator<Item = (u32, Option<D>)>> From<I> for ChangeSet<D> {
    fn from(blocks: I) -> Self {
        Self {
            blocks: blocks.into_iter().collect(),
        }
    }
}

impl<D> FromIterator<(u32, Option<D>)> for ChangeSet<D> {
    fn from_iter<T: IntoIterator<Item = (u32, Option<D>)>>(iter: T) -> Self {
        Self {
            blocks: iter.into_iter().collect(),
        }
    }
}

impl<D> FromIterator<(u32, D)> for ChangeSet<D> {
    fn from_iter<T: IntoIterator<Item = (u32, D)>>(iter: T) -> Self {
        Self {
            blocks: iter
                .into_iter()
                .map(|(height, hash)| (height, Some(hash)))
                .collect(),
        }
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
/// On success, a tuple is returned ([`CheckPoint`], [`ChangeSet`]).
///
/// # Errors
///
/// [`CannotConnectError`] occurs when the `original_tip` and `update_tip` chains are disjoint:
///
/// - If no point of agreement is found between the update and original chains.
/// - A point of agreement is found but the update is ambiguous above the point of agreement (a.k.a.
///   the update and original chain both have a block above the point of agreement, but their
///   heights do not overlap).
/// - The update attempts to replace the genesis block of the original chain.
fn merge_chains<D>(
    original_tip: CheckPoint<D>,
    update_tip: CheckPoint<D>,
) -> Result<(CheckPoint<D>, ChangeSet<D>), CannotConnectError>
where
    D: ToBlockHash + fmt::Debug + Copy,
{
    let mut changeset = ChangeSet::<D>::default();

    let mut orig = original_tip.iter();
    let mut update = update_tip.iter();

    let mut curr_orig = None;
    let mut curr_update = None;

    let mut prev_orig: Option<CheckPoint<D>> = None;
    let mut prev_update: Option<CheckPoint<D>> = None;

    let mut point_of_agreement_found = false;

    let mut prev_orig_was_invalidated = false;

    let mut potentially_invalidated_heights = vec![];

    // If we can, we want to return the update tip as the new tip because this allows checkpoints
    // in multiple locations to keep the same `Arc` pointers when they are being updated from each
    // other using this function. We can do this as long as the update contains every
    // block's height of the original chain.
    let mut is_update_height_superset_of_original = true;

    // To find the difference between the new chain and the original we iterate over both of them
    // from the tip backwards in tandem. We are always dealing with the highest one from either
    // chain first and move to the next highest. The crucial logic is applied when they have
    // blocks at the same height.
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
                changeset.blocks.insert(u.height(), Some(u.data()));
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
                    // invalidated (if it exists). This ensures that there is an unambiguous point
                    // of connection to the original chain from the update chain
                    // (i.e. we know the precisely which original blocks are
                    // invalid).
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
                    if o.eq_ptr(u) {
                        if is_update_height_superset_of_original {
                            return Ok((update_tip, changeset));
                        } else {
                            let new_tip = apply_changeset_to_checkpoint(original_tip, &changeset)
                                .map_err(|_| CannotConnectError {
                                try_include_height: 0,
                            })?;
                            return Ok((new_tip, changeset));
                        }
                    }
                } else {
                    // We have an invalidation height so we set the height to the updated hash and
                    // also purge all the original chain block hashes above this block.
                    changeset.blocks.insert(u.height(), Some(u.data()));
                    for invalidated_height in potentially_invalidated_heights.drain(..) {
                        changeset.blocks.insert(invalidated_height, None);
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

    let new_tip = apply_changeset_to_checkpoint(original_tip, &changeset).map_err(|_| {
        CannotConnectError {
            try_include_height: 0,
        }
    })?;
    Ok((new_tip, changeset))
}
