use core::fmt;
use core::ops::RangeBounds;

use alloc::sync::Arc;
use bitcoin::{block::Header, BlockHash};

use crate::BlockId;

/// Interval for skiplist pointers based on checkpoint index.
const CHECKPOINT_SKIP_INTERVAL: u32 = 100;

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
    /// Skip pointer for fast traversals.
    skip: Option<Arc<CPInner<D>>>,
    /// Index of this checkpoint (number of checkpoints from the first).
    index: u32,
}

/// When a `CPInner` is dropped we need to go back down the chain and manually remove any
/// no-longer referenced checkpoints. Letting the default rust dropping mechanism handle this
/// leads to recursive logic and stack overflows
///
/// https://github.com/bitcoindevkit/bdk/issues/1634
impl<D> Drop for CPInner<D> {
    fn drop(&mut self) {
        // Take out `prev` so its `drop` won't be called when this drop is finished
        let mut current = self.prev.take();
        while let Some(arc_node) = current {
            // Get rid of the Arc around `prev` if we're the only one holding a ref
            // So the `drop` on it won't be called when the `Arc` is dropped.
            //
            // FIXME: When MSRV > 1.70.0 this should use Arc::into_inner which actually guarantees
            // that no recursive drop calls can happen even with multiple threads.
            match Arc::try_unwrap(arc_node).ok() {
                Some(mut node) => {
                    // Keep going backwards
                    current = node.prev.take();
                    // Don't call `drop` on `CPInner` since that risks it becoming recursive.
                    core::mem::forget(node);
                }
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

    /// Get the index of this checkpoint (number of checkpoints from the first).
    pub fn index(&self) -> u32 {
        self.0.index
    }

    /// Get the skip pointer checkpoint if it exists.
    pub fn skip(&self) -> Option<CheckPoint<D>> {
        self.0.skip.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter<D> {
        self.clone().into_iter()
    }

    /// Get checkpoint at `height`.
    ///
    /// Returns `None` if checkpoint at `height` does not exist`.
    pub fn get(&self, height: u32) -> Option<Self> {
        let mut current = self.clone();

        if current.height() == height {
            return Some(current);
        }

        // Use skip pointers to jump close to target
        while current.height() > height {
            match current.skip() {
                Some(skip_cp) => match skip_cp.height().cmp(&height) {
                    core::cmp::Ordering::Greater => current = skip_cp,
                    core::cmp::Ordering::Equal => return Some(skip_cp),
                    core::cmp::Ordering::Less => break, // Skip would undershoot
                },
                None => break, // No more skip pointers
            }
        }

        // Linear search for exact height
        while current.height() > height {
            match current.prev() {
                Some(prev_cp) => match prev_cp.height().cmp(&height) {
                    core::cmp::Ordering::Greater => current = prev_cp,
                    core::cmp::Ordering::Equal => return Some(prev_cp),
                    core::cmp::Ordering::Less => break, // Height doesn't exist
                },
                None => break, // End of chain
            }
        }

        None
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

        let is_above_bound = |height: u32| match end_bound {
            core::ops::Bound::Included(inc_bound) => height > inc_bound,
            core::ops::Bound::Excluded(exc_bound) => height >= exc_bound,
            core::ops::Bound::Unbounded => false,
        };

        let mut current = self.clone();

        // Use skip pointers to jump close to target
        while is_above_bound(current.height()) {
            match current.skip() {
                Some(skip_cp) if is_above_bound(skip_cp.height()) => {
                    current = skip_cp;
                }
                _ => break, // Skip would undershoot or doesn't exist
            }
        }

        // Linear search to exact position
        while is_above_bound(current.height()) {
            match current.prev() {
                Some(prev) => current = prev,
                None => break,
            }
        }

        // Iterate from start point
        current.into_iter().take_while(move |cp| match start_bound {
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
    D: ToBlockHash + fmt::Debug + Copy,
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
            skip: None,
            index: 0,
        }))
    }

    /// Construct from an iterator of block data.
    ///
    /// Returns `Err(None)` if `blocks` doesn't yield any data. If the blocks are not in ascending
    /// height order, then returns an `Err(..)` containing the last checkpoint that would have been
    /// extended.
    pub fn from_blocks(blocks: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Option<Self>> {
        let mut blocks = blocks.into_iter();
        let (height, data) = blocks.next().ok_or(None)?;
        let mut cp = CheckPoint::new(height, data);
        cp = cp.extend(blocks)?;

        Ok(cp)
    }

    /// Extends the checkpoint linked list by a iterator containing `height` and `data`.
    ///
    /// Returns an `Err(self)` if there is block which does not have a greater height than the
    /// previous one.
    pub fn extend(self, blockdata: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Self> {
        let mut cp = self.clone();
        for (height, data) in blockdata {
            cp = cp.push(height, data)?;
        }
        Ok(cp)
    }

    /// Inserts `data` at its `height` within the chain.
    ///
    /// The effect of `insert` depends on whether a height already exists. If it doesn't, the data
    /// we inserted and all pre-existing entries higher than it will be re-inserted after it. If the
    /// height already existed and has a conflicting block hash then it will be purged along with
    /// all entries following it. The returned chain will have a tip of the data passed in. Of
    /// course, if the data was already present then this just returns `self`.
    ///
    /// # Panics
    ///
    /// This panics if called with a genesis block that differs from that of `self`.
    #[must_use]
    pub fn insert(self, height: u32, data: D) -> Self {
        let mut cp = self.clone();
        let mut tail = vec![];
        let base = loop {
            if cp.height() == height {
                if cp.hash() == data.to_blockhash() {
                    return self;
                }
                assert_ne!(cp.height(), 0, "cannot replace genesis block");
                // If we have a conflict we just return the inserted data because the tail is by
                // implication invalid.
                tail = vec![];
                break cp.prev().expect("can't be called on genesis block");
            }

            if cp.height() < height {
                break cp;
            }

            tail.push((cp.height(), cp.data()));
            cp = cp.prev().expect("will break before genesis block");
        };

        // Rebuild the chain: base -> new block -> tail
        base.extend(core::iter::once((height, data)).chain(tail.into_iter().rev()))
            .expect("tail is in order")
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the
    /// one you are pushing on to.
    pub fn push(self, height: u32, data: D) -> Result<Self, Self> {
        if self.height() >= height {
            return Err(self);
        }

        let new_index = self.0.index + 1;

        // Skip pointers are added every CHECKPOINT_SKIP_INTERVAL (100) checkpoints
        // e.g., checkpoints at index 100, 200, 300, etc. have skip pointers
        let needs_skip_pointer =
            new_index >= CHECKPOINT_SKIP_INTERVAL && new_index % CHECKPOINT_SKIP_INTERVAL == 0;

        let skip = if needs_skip_pointer {
            // Skip pointer points back CHECKPOINT_SKIP_INTERVAL positions
            // e.g., checkpoint at index 200 points to checkpoint at index 100
            // We walk back CHECKPOINT_SKIP_INTERVAL - 1 steps since we start from self (index new_index - 1)
            let mut current = self.0.clone();
            for _ in 0..(CHECKPOINT_SKIP_INTERVAL - 1) {
                // This is safe: if we're at index >= 100, we must have at least 99 predecessors
                current = current.prev.clone().expect("chain has enough checkpoints");
            }
            Some(current)
        } else {
            None
        };

        Ok(Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: Some(self.0),
            skip,
            index: new_index,
        })))
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
