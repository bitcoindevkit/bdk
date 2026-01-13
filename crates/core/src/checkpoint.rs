use alloc::sync::Arc;
use alloc::vec::Vec;
use bitcoin::{block::Header, BlockHash};
use core::fmt;
use core::ops::RangeBounds;

use crate::{BlockId, CheckPointEntry, CheckPointEntryIter};

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

    /// Returns `None` if the type has no knowledge of the previous [`BlockHash`].
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

impl<D> CheckPoint<D>
where
    D: ToBlockHash,
{
    /// Iterate entries from this checkpoint in descending height.
    pub fn entry_iter(&self) -> CheckPointEntryIter<D> {
        self.to_entry().into_iter()
    }

    /// Transforms this checkpoint into a [`CheckPointEntry`].
    pub fn into_entry(self) -> CheckPointEntry<D> {
        CheckPointEntry::Occupied(self)
    }

    /// Creates a [`CheckPointEntry`].
    pub fn to_entry(&self) -> CheckPointEntry<D> {
        CheckPointEntry::Occupied(self.clone())
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
        // Step 1: Split chain into base (everything below height) and tail (height and above).
        // We collect the tail to re-insert after placing in our new `data`.
        let mut base_opt = Some(self.clone());
        let mut tail = Vec::<Self>::new();
        for cp in self.iter().take_while(|cp| cp.height() > height) {
            base_opt = cp.prev();
            tail.push(cp);
        }
        // Insert the new `data`.
        let index = tail.partition_point(|cp| cp.height() > height);
        tail.insert(index, CheckPoint::new(height, data));

        // Step 2: Rebuild the chain by pushing each element from tail onto base.
        // This process handles conflicts automatically through push's validation.
        while let Some(tail_cp) = tail.pop() {
            let base = match base_opt {
                Some(base) => base,
                None => {
                    // We need a base to push stuff on.
                    base_opt = Some(tail_cp);
                    continue;
                }
            };

            match base.push(tail_cp.height(), tail_cp.data()) {
                Ok(new_base) => {
                    base_opt = Some(new_base);
                    continue;
                }
                Err(base) => {
                    // Is this the height of insertion?
                    if tail_cp.height() == height {
                        // Failed due to prev_blockhash conflict.
                        if base.height() + 1 == tail_cp.height() {
                            // Displace the conflicting cp with the new cp.
                            // Clear tail as those are now orphaned.
                            base_opt = base.prev();
                            tail.clear();
                            tail.push(tail_cp);
                            continue;
                        }

                        // Failed because height already exists.
                        if tail_cp.height() == base.height() {
                            base_opt = base.prev();
                            if base.hash() == tail_cp.hash() {
                                // If hash matches, this `insert` is a noop.
                                return self;
                            }
                            // Hash conflicts: purge everything above.
                            tail.clear();
                            tail.push(tail_cp);
                            continue;
                        }
                    }

                    if tail_cp.height() > height {
                        // Push failed for checkpoint above our insertion.
                        // This means the data above our insertion got orphaned.
                        base_opt = Some(base);
                        break;
                    }

                    unreachable!(
                        "fail cannot be a result of pushing something that was part of `self`"
                    );
                }
            }
        }

        base_opt.expect("must have atleast one checkpoint")
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if:
    /// * The block you are pushing on is not at a greater height that the one you are pushing on to.
    /// * The `prev_blockhash` does not match.
    pub fn push(self, height: u32, data: D) -> Result<Self, Self> {
        // Reject if trying to push at or below current height - chain must grow forward.
        if height <= self.height() {
            return Err(self);
        }

        // For contiguous height, ensure prev_blockhash does not conflict.
        if let Some(prev_blockhash) = data.prev_blockhash() {
            if self.height() + 1 == height && self.hash() != prev_blockhash {
                return Err(self);
            }
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
}

/// Iterates over checkpoints backwards.
pub struct CheckPointIter<D> {
    next: Option<Arc<CPInner<D>>>,
}

impl<D> Iterator for CheckPointIter<D> {
    type Item = CheckPoint<D>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next.clone()?;
        self.next.clone_from(&current.prev);
        Some(CheckPoint(current))
    }
}

impl<D> IntoIterator for CheckPoint<D> {
    type Item = CheckPoint<D>;
    type IntoIter = CheckPointIter<D>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter { next: Some(self.0) }
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
