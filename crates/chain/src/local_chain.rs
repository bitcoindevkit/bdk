//! The [`LocalChain`] is a local implementation of [`ChainOracle`].

use core::convert::Infallible;

use crate::collections::BTreeMap;
use crate::{BlockId, ChainOracle};
use alloc::sync::Arc;
use bitcoin::BlockHash;

/// A structure that represents changes to [`LocalChain`].
pub type ChangeSet = BTreeMap<u32, Option<BlockHash>>;

/// A block of [`LocalChain`].
///
/// Blocks are presented in a linked-list.
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

/// A safe representation of the underlying raw pointer of a [`CheckPoint`].
///
/// If two [`CheckPoint`]s return [`Pointer`]s that are equal, then the underlying raw pointer of
/// the checkpoints are equal.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct Pointer(*const CPInner);

impl CheckPoint {
    /// Construct a [`CheckPoint`] from a [`BlockId`].
    pub fn new(block: BlockId) -> Self {
        Self(Arc::new(CPInner { block, prev: None }))
    }

    /// Extends [`CheckPoint`] with `block` and returns the new checkpoint tip.
    ///
    /// Returns an `Err` of the initial checkpoint
    pub fn extend(self, block: BlockId) -> Result<Self, Self> {
        if self.height() < block.height {
            Ok(Self(Arc::new(CPInner {
                block,
                prev: Some(self.0),
            })))
        } else {
            Err(self)
        }
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

    /// Get the previous checkpoint.
    pub fn prev(&self) -> Option<CheckPoint> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter {
        CheckPointIter {
            current: Some(Arc::clone(&self.0)),
        }
    }

    /// Returns a safe representation of the underlying raw pointer of a [`CheckPoint`].
    ///
    /// See [`Pointer`] to learn more.
    pub fn as_ptr(&self) -> Pointer {
        Pointer(Arc::as_ptr(&self.0))
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
                    let mut prev = Option::<CheckPoint>::None;
                    move |(height, hash)| {
                        let cp = match prev.clone() {
                            Some(prev) => {
                                prev.extend(BlockId { height, hash }).expect("must extend")
                            }
                            None => CheckPoint::new(BlockId { height, hash }),
                        };
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

    /// Updates [`Self`] with the given `new_tip`.
    ///
    /// The method returns [`ChangeSet`] on success. This represents the applied changes to
    /// [`Self`].
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
    pub fn update(&mut self, new_tip: CheckPoint) -> Result<ChangeSet, CannotConnectError> {
        match self.tip() {
            Some(original_tip) => {
                let (cp, changeset) = merge(original_tip, new_tip)?;
                *self = Self::from_checkpoint(cp);
                Ok(changeset)
            }
            None => {
                let mut changeset = ChangeSet::default();
                for cp in new_tip.iter() {
                    let block = cp.block_id();
                    changeset.insert(block.height, Some(block.hash));
                    self.checkpoints.insert(block.height, cp.clone());
                }
                Ok(changeset)
            }
        }
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

    /// Insert a [`BlockId`].
    ///
    /// # Errors
    ///
    /// Replacing the block hash of an existing checkpoint will result in an error.
    pub fn insert_block(&mut self, block_id: BlockId) -> Result<ChangeSet, InsertBlockError> {
        use crate::collections::btree_map::Entry;

        match self.checkpoints.entry(block_id.height) {
            Entry::Vacant(entry) => {
                entry.insert(CheckPoint::new(block_id));
                self.fix_links(block_id.height);
                Ok(core::iter::once((block_id.height, Some(block_id.hash))).collect())
            }
            Entry::Occupied(entry) => {
                let cp = entry.get();
                if cp.block_id() == block_id {
                    Ok(ChangeSet::default())
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

    /// Internal method for fixing pointers to make checkpoints a properly linked list. I.e.
    /// [`CheckPoint::prev`] should return the previous checkpoint.
    ///
    /// We fix checkpoints from `start_height` and higher.
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

fn merge(
    original_tip: CheckPoint,
    update_tip: CheckPoint,
) -> Result<(CheckPoint, ChangeSet), CannotConnectError> {
    let mut o_visited = BTreeMap::<u32, CheckPoint>::new();
    let mut o_iter = original_tip
        .iter()
        .inspect(|cp| {
            o_visited.insert(cp.height(), cp.clone());
        })
        .peekable();

    let mut u_visited = BTreeMap::<u32, CheckPoint>::new();
    let u_iter = update_tip.iter().inspect(|cp| {
        u_visited.insert(cp.height(), cp.clone());
    });

    let mut highest_agreement = Option::<u32>::None;

    for u_cp in u_iter {
        // match heights of `o_iter` and `u_iter`
        let o_cp = loop {
            match o_iter.peek() {
                Some(o_cp) if o_cp.height() > u_cp.height() => o_iter.next(),
                Some(o_cp) if o_cp.height() == u_cp.height() => break o_iter.next(),
                _ => break None,
            };
        };
        let o_cp = match o_cp {
            Some(o_cp) => o_cp,
            None => continue,
        };

        // perfect match!
        if o_cp.as_ptr() == u_cp.as_ptr() {
            let invalidate_lb = o_cp.height() + 1;
            if let Some(invalidate_o) = o_visited.range(invalidate_lb..).next().map(|(&h, _)| h) {
                if let Some(invalidate_u) = u_visited.range(invalidate_o..).next().map(|(&h, _)| h)
                {
                    if invalidate_u != invalidate_o {
                        return Err(CannotConnectError {
                            try_include_height: invalidate_o,
                        });
                    }
                }
            }
            let changeset = u_visited
                .split_off(&invalidate_lb)
                .into_iter()
                .map(|(h, cp)| (h, Some(cp.hash())))
                .collect::<ChangeSet>();
            return Ok((update_tip, changeset));
        }

        // find highest agreement height
        if highest_agreement.is_none() && o_cp.hash() == u_cp.hash() {
            highest_agreement = Some(o_cp.height());
        }
    }

    // check invalidation
    let mut explicit_invalidation = false;
    let invalidate_lb = match highest_agreement {
        Some(h) => {
            let invalidate_lb = h + 1;
            if let Some(invalidate_o) = o_visited.range(invalidate_lb..).next().map(|(&h, _)| h) {
                if let Some(invalidate_u) = u_visited.range(invalidate_o..).next().map(|(&h, _)| h)
                {
                    explicit_invalidation = true;
                    if invalidate_u != invalidate_o {
                        return Err(CannotConnectError {
                            try_include_height: invalidate_o,
                        });
                    }
                }
            }
            invalidate_lb
        }
        None => {
            explicit_invalidation = true;
            // If there is no agreement height, the lowest original checkpoint must be displaced by
            // an update checkpoint. We can ensure this if `o_iter` is exhausted and the first
            // height of `o_visited` exists in `u_visited`.
            if let Some(cp) = o_iter.peek() {
                return Err(CannotConnectError {
                    try_include_height: cp.height(),
                });
            }
            let first_o = o_visited
                .keys()
                .next()
                .expect("must atleast have one height");
            if !u_visited.contains_key(first_o) {
                return Err(CannotConnectError {
                    try_include_height: *first_o,
                });
            }
            // we invalidate everything in the original chain
            0
        }
    };

    // make changeset
    let changeset = {
        let mut changeset = match explicit_invalidation {
            true => o_visited
                .range(invalidate_lb..)
                .map(|(&h, _)| (h, None))
                .collect::<ChangeSet>(),
            false => ChangeSet::default(),
        };
        for (h, u_cp) in &u_visited {
            match o_visited.get(h) {
                Some(o_cp) if o_cp.hash() == u_cp.hash() => continue,
                _ => changeset.insert(*h, Some(u_cp.hash())),
            };
        }
        changeset
    };

    // get original cp before the first change
    let first_change_height = match changeset.keys().next() {
        Some(&h) => h,
        // empty changeset
        None => return Ok((original_tip, changeset)),
    };

    let start_cp = o_visited
        .range(..first_change_height)
        .next_back()
        .map(|(_, cp)| cp.clone());

    let chain_ext = {
        let mut chain_ext = o_visited
            .range(first_change_height..)
            .map(|(&h, cp)| (h, cp.hash()))
            .collect::<BTreeMap<u32, BlockHash>>();
        for (&height, &hash_delta) in &changeset {
            match hash_delta {
                Some(hash) => chain_ext.insert(height, hash),
                None => chain_ext.remove(&height),
            };
        }
        chain_ext
    };

    // build new chain
    let mut cp = start_cp;
    for (height, hash) in chain_ext {
        let block = BlockId { height, hash };
        match cp.clone() {
            Some(this_cp) => cp = Some(this_cp.extend(block).expect("must extend")),
            _ => {
                let _ = cp.insert(CheckPoint::new(block));
            }
        };
    }

    Ok((cp.expect("must have checkpoint"), changeset))
}
