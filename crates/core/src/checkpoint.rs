use core::fmt;
use core::ops::RangeBounds;

use alloc::sync::Arc;
use bitcoin::{block::Header, BlockHash};

use crate::BlockId;

/// A checkpoint is a node of a reference-counted linked list of [`BlockId`]s.
///
/// Checkpoints are cheaply cloneable and are useful to find the agreement point between two sparse
/// block chains.
#[derive(Debug)]
pub struct CheckPoint<D = BlockHash>(Arc<CPInner<D>>);

impl<D> Clone for CheckPoint<D> {
    fn clone(&self) -> Self {
        CheckPoint(Arc::clone(&self.0))
    }
}

/// The internal contents of [`CheckPoint`].
#[derive(Debug)]
struct CPInner<D> {
    /// Block id
    block_id: BlockId,
    /// Data.
    data: D,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner<D>>>,
}

/// When a `CPInner` is dropped we need to go back down the chain and manually remove any
/// no-longer referenced checkpoints. Letting the default rust dropping mechanism handle this
/// leads to recursive logic and stack overflows
///
/// https://github.com/bitcoindevkit/bdk/issues/1634
impl<D> Drop for CPInner<D> {
    fn drop(&mut self) {
        // Take out `prev` so its `drop` won't be called when this drop is finished.
        let mut current = self.prev.take();
        // Collect nodes to drop later so we avoid recursive drop calls while not leaking memory.
        while let Some(arc_node) = current {
            // Get rid of the `Arc` around `prev` if we're the only one holding a reference so the
            // `drop` on it won't be called when the `Arc` is dropped.
            let arc_inner = Arc::into_inner(arc_node);

            match arc_inner {
                // Keep going backwards.
                Some(mut node) => current = node.prev.take(),
                None => break,
            }
        }
    }
}

/// Trait that converts [`CheckPoint`] `data` to [`BlockHash`].
///
/// Implementations of [`ToBlockHash`] must always return the block’s consensus-defined hash. If
/// your type contains extra fields (timestamps, metadata, etc.), these must be ignored. For
/// example, [`BlockHash`] trivially returns itself, [`Header`] calls its `block_hash()`, and a
/// wrapper type around a [`Header`] should delegate to the header’s hash rather than derive one
/// from other fields.
pub trait ToBlockHash {
    /// Returns the [`BlockHash`] for the associated [`CheckPoint`] `data` type.
    fn to_blockhash(&self) -> BlockHash;

    /// Returns the previous [`BlockHash`] of the associated [`CheckPoint::data`] type if known.
    ///
    /// This has a default implementation that returns `None`. Implementors are expected to override
    /// this if the previous block hash is known.
    fn prev_blockhash(&self) -> Option<BlockHash> {
        None
    }
}

impl ToBlockHash for BlockHash {
    fn to_blockhash(&self) -> BlockHash {
        *self
    }
}

impl ToBlockHash for Header {
    fn to_blockhash(&self) -> BlockHash {
        self.block_hash()
    }

    fn prev_blockhash(&self) -> Option<BlockHash> {
        Some(self.prev_blockhash)
    }
}

impl<D> PartialEq for CheckPoint<D> {
    fn eq(&self, other: &Self) -> bool {
        let self_cps = self.iter().map(|cp| cp.block_id());
        let other_cps = other.iter().map(|cp| cp.block_id());
        self_cps.eq(other_cps)
    }
}

// Methods for any `D`
impl<D> CheckPoint<D> {
    /// Get a reference of the `data` of the checkpoint.
    pub fn data_ref(&self) -> &D {
        &self.0.data
    }

    /// Get the `data` of a the checkpoint.
    pub fn data(&self) -> D
    where
        D: Clone,
    {
        self.0.data.clone()
    }

    /// Get the [`BlockId`] of the checkpoint.
    pub fn block_id(&self) -> BlockId {
        self.0.block_id
    }

    /// Get the `height` of the checkpoint.
    pub fn height(&self) -> u32 {
        self.block_id().height
    }

    /// Get the block hash of the checkpoint.
    pub fn hash(&self) -> BlockHash {
        self.block_id().hash
    }

    /// Get the previous checkpoint in the chain.
    pub fn prev(&self) -> Option<CheckPoint<D>> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter<D> {
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
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint<D>>
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

    /// Returns the checkpoint at `height` if one exists, otherwise the nearest checkpoint at a
    /// lower height.
    ///
    /// This is equivalent to taking the "floor" of `height` over this checkpoint chain.
    ///
    /// Returns `None` if no checkpoint exists at or below the given height.
    pub fn floor_at(&self, height: u32) -> Option<Self> {
        self.range(..=height).next()
    }

    /// Returns the checkpoint located a number of heights below this one.
    ///
    /// This is a convenience wrapper for [`CheckPoint::floor_at`], subtracting `to_subtract` from
    /// the current height.
    ///
    /// - If a checkpoint exists exactly `offset` heights below, it is returned.
    /// - Otherwise, the nearest checkpoint *below that target height* is returned.
    ///
    /// Returns `None` if `to_subtract` is greater than the current height, or if there is no
    /// checkpoint at or below the target height.
    pub fn floor_below(&self, offset: u32) -> Option<Self> {
        self.floor_at(self.height().checked_sub(offset)?)
    }

    /// This method tests for `self` and `other` to have equal internal pointers.
    pub fn eq_ptr(&self, other: &Self) -> bool {
        Arc::as_ptr(&self.0) == Arc::as_ptr(&other.0)
    }
}

// Methods where `D: ToBlockHash`
impl<D> CheckPoint<D>
where
    D: ToBlockHash + fmt::Debug + Clone,
{
    /// Construct a new base [`CheckPoint`] from given `height` and `data` at the front of a linked
    /// list.
    pub fn new(height: u32, data: D) -> Self {
        Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: None,
        }))
    }

    /// Construct from an iterator of block data.
    ///
    /// Returns `Err(None)` if `blocks` doesn't yield any data. If the blocks are inconsistent
    /// or are not in ascending height order, then returns an `Err(..)` containing the last
    /// checkpoint that would have been extended.
    pub fn from_blocks(blocks: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Option<Self>> {
        let mut blocks = blocks.into_iter();
        let (height, data) = blocks.next().ok_or(None)?;
        let mut cp = CheckPoint::new(height, data);
        cp = cp.extend(blocks)?;

        Ok(cp)
    }

    /// Extends the checkpoint linked list by a iterator containing `height` and `data`.
    ///
    /// Returns an `Err(self)` if there is a block which does not have a greater height than the
    /// previous one, or doesn't properly link to an adjacent block via its `prev_blockhash`.
    /// See docs for [`CheckPoint::push`].
    pub fn extend(self, blockdata: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Self> {
        let mut cp = self.clone();
        for (height, data) in blockdata {
            cp = cp.push(height, data)?;
        }
        Ok(cp)
    }

    /// Inserts `data` at its `height` within the chain.
    ///
    /// This method always returns a valid [`CheckPoint`], handling conflicts through displacement
    /// and eviction:
    ///
    /// ## Rules
    ///
    /// ### No Conflict
    /// If the inserted `data` doesn't conflict with existing checkpoints, it's inserted normally
    /// into the chain. The new checkpoint is added at the specified height and all existing
    /// checkpoints remain unchanged. If a node exists at the specified height and the inserted
    /// data's `to_blockhash` matches the existing node's hash, then no change occurs and this
    /// function returns `self`. That assumes that equality of block hashes implies equality
    /// of the block data.
    ///
    /// ### Displacement
    /// When `data.prev_blockhash()` conflicts with a checkpoint's hash, that checkpoint is
    /// omitted from the new chain. This happens when the inserted block references
    /// a different previous block than what exists in the chain.
    ///
    /// ### Eviction
    /// When the inserted `data.to_blockhash()` conflicts with a higher checkpoint's
    /// `prev_blockhash`, that checkpoint and all checkpoints above it are removed.
    /// This occurs when the new block's hash doesn't match what higher blocks expect as their
    /// previous block, making those higher blocks invalid.
    ///
    /// ### Combined Displacement and Eviction
    /// Both displacement and eviction can occur when inserting a block that conflicts with
    /// both lower and higher checkpoints in the chain. The lower conflicting checkpoint gets
    /// displaced while higher conflicting checkpoints get purged.
    ///
    /// # Parameters
    ///
    /// - `height`: The block height where `data` should be inserted
    /// - `data`: The block data to insert (must implement [`ToBlockHash`])
    ///
    /// # Panics
    ///
    /// Panics if the insertion would replace (or omit) the checkpoint at height 0 (a.k.a
    /// "genesis"). Although [`CheckPoint`] isn't structurally required to contain a genesis
    /// block, if one is present, it stays immutable and can't be replaced.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use bdk_core::CheckPoint;
    /// # use bitcoin::hashes::Hash;
    /// # use bitcoin::BlockHash;
    /// let cp = CheckPoint::new(100, BlockHash::all_zeros());
    ///
    /// // Insert at new height - no conflicts
    /// let cp = cp.insert(101, BlockHash::all_zeros());
    /// assert_eq!(cp.height(), 101);
    ///
    /// // Insert at existing height with same data - no change
    /// let cp2 = cp.clone().insert(100, BlockHash::all_zeros());
    /// assert_eq!(cp2, cp);
    ///
    /// // Replace the data at height 100 - higher nodes are evicted
    /// let cp = cp.insert(100, Hash::hash(b"block_100_new"));
    /// assert_eq!(cp.height(), 100);
    /// ```
    #[must_use]
    pub fn insert(self, height: u32, data: D) -> Self {
        let mut cp = self.clone();
        let mut tail = vec![];
        let mut new_cp = loop {
            if cp.height() == height {
                if cp.hash() == data.to_blockhash() {
                    return self;
                }
                assert_ne!(cp.height(), 0, "cannot replace the genesis block");
                // We're replacing an entry, so the tail must be invalid.
                tail.clear();
            }
            if cp.height() > height {
                tail.push((cp.height(), cp.data()));
            }
            if cp.height() < height && !cp.is_conflicted_by_next(height, &data) {
                break cp;
            }
            match cp.prev() {
                Some(prev) => cp = prev,
                None => {
                    // We didnt locate a base, so start a new `CheckPoint` with the inserted
                    // data at the front. We only require that the node at height 0
                    // isn't wrongfully omitted.
                    assert_ne!(cp.height(), 0, "cannot replace the genesis block");
                    break CheckPoint::new(height, data.clone());
                }
            }
        };
        // Reconstruct the new chain. If `push` errors, return the best non-conflicted checkpoint.
        let base_height = new_cp.height();
        for (height, data) in core::iter::once((height, data))
            .chain(tail.into_iter().rev())
            .skip_while(|(height, _)| *height <= base_height)
        {
            match new_cp.clone().push(height, data) {
                Ok(cp) => new_cp = cp,
                Err(cp) => return cp,
            }
        }
        new_cp
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// # Errors
    ///
    /// - Returns an `Err(self)` if the block you are pushing on is not at a greater height that the
    ///   one you are pushing on to.
    /// - If this checkpoint would be conflicted by the inserted data, i.e. the hash of this
    ///   checkpoint doesn't agree with the data's `prev_blockhash`, then returns `Err(self)`.
    pub fn push(self, height: u32, data: D) -> Result<Self, Self> {
        // `height` must be greater than self height.
        if self.height() >= height {
            return Err(self);
        }
        // The pushed data must not conflict with self.
        if self.is_conflicted_by_next(height, &data) {
            return Err(self);
        }
        Ok(Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: Some(self.0),
        })))
    }

    /// Whether `self` would be conflicted by the addition of `data` at the specfied `height`.
    ///
    /// If `true`, this [`CheckPoint`] must reject an attempt to extend it with the given `data`
    /// at the target `height`.
    ///
    /// To be consistent the chain must have the following property:
    ///
    /// If the target `height` is greater by 1, then the block hash of `self` equals the data's
    /// `prev_blockhash`, otherwise a conflict is detected.
    fn is_conflicted_by_next(&self, height: u32, data: &D) -> bool {
        self.height().saturating_add(1) == height
            && data
                .prev_blockhash()
                .is_some_and(|prev_hash| prev_hash != self.hash())
    }
}

/// Iterates over checkpoints backwards.
pub struct CheckPointIter<D> {
    current: Option<Arc<CPInner<D>>>,
}

impl<D> Iterator for CheckPointIter<D> {
    type Item = CheckPoint<D>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.clone()?;
        self.current.clone_from(&current.prev);
        Some(CheckPoint(current))
    }
}

impl<D> IntoIterator for CheckPoint<D> {
    type Item = CheckPoint<D>;
    type IntoIter = CheckPointIter<D>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter {
            current: Some(self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make sure that dropping checkpoints does not result in recursion and stack overflow.
    #[test]
    fn checkpoint_drop_is_not_recursive() {
        let run = || {
            let mut cp = CheckPoint::new(0, bitcoin::hashes::Hash::hash(b"genesis"));

            for height in 1u32..=(1024 * 10) {
                let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
                cp = cp.push(height, hash).unwrap();
            }

            // `cp` would be dropped here.
        };
        std::thread::Builder::new()
            // Restrict stack size.
            .stack_size(32 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn checkpoint_does_not_leak() {
        let mut cp = CheckPoint::new(0, bitcoin::hashes::Hash::hash(b"genesis"));

        for height in 1u32..=1000 {
            let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
            cp = cp.push(height, hash).unwrap();
        }

        let genesis = cp.get(0).expect("genesis exists");
        let weak = Arc::downgrade(&genesis.0);

        // At this point there should be exactly two strong references to the
        // genesis checkpoint: the variable `genesis` and the chain `cp`.
        assert_eq!(
            Arc::strong_count(&genesis.0),
            2,
            "`cp` and `genesis` should be the only strong references",
        );

        // Dropping the chain should remove one strong reference.
        drop(cp);
        assert_eq!(
            Arc::strong_count(&genesis.0),
            1,
            "`genesis` should be the last strong reference after `cp` is dropped",
        );

        // Dropping the final reference should deallocate the node, so the weak
        // reference cannot be upgraded.
        drop(genesis);
        assert!(
            weak.upgrade().is_none(),
            "the checkpoint node should be freed when all strong references are dropped",
        );
    }
}
