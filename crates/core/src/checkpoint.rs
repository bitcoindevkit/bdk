use core::fmt;
use core::ops::RangeBounds;

use alloc::sync::Arc;
use alloc::vec::Vec;
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
    /// Data (if any).
    data: Option<D>,
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
    /// Get a reference of the `data` of the checkpoint if it exists.
    pub fn data_ref(&self) -> Option<&D> {
        self.0.data.as_ref()
    }

    /// Get the `data` of the checkpoint if it exists.
    pub fn data(&self) -> Option<D>
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

    /// Finds the checkpoint with `data` at `height` if one exists, otherwise the neareast
    /// checkpoint with `data` at a lower height.
    ///
    /// This is equivalent to taking the “floor” of "height" over this checkpoint chain, filtering
    /// out any placeholder entries that do not contain any `data`.
    ///
    /// Returns `None` if no checkpoint with `data` exists at or below the given height.
    pub fn find_data(&self, height: u32) -> Option<Self> {
        self.range(..=height).find(|cp| cp.data_ref().is_some())
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
    ///
    /// If `data` contains previous block via [`ToBlockHash::prev_blockhash`], this will also create
    /// a placeholder checkpoint at `height - 1` with that hash and with `data: None`, and link the
    /// new checkpoint to it. The placeholder can be materialized later by inserting data at its
    /// height.
    pub fn new(height: u32, data: D) -> Self {
        // If `data` has a `prev_blockhash`, create a placeholder checkpoint one height below.
        let prev = if height > 0 {
            match data.prev_blockhash() {
                Some(prev_blockhash) => Some(Arc::new(CPInner {
                    block_id: BlockId {
                        height: height - 1,
                        hash: prev_blockhash,
                    },
                    data: None,
                    prev: None,
                })),
                None => None,
            }
        } else {
            None
        };

        Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data: Some(data),
            prev,
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
    /// This method maintains chain consistency by ensuring that all blocks are properly linked
    /// through their `prev_blockhash` relationships. When conflicts are detected, checkpoints
    /// are either displaced (converted to placeholders) or purged to maintain a valid chain.
    ///
    /// ## Behavior
    ///
    /// The insertion process follows these rules:
    ///
    /// 1. **If inserting at an existing height with the same hash:**
    ///    - Placeholder checkpoint: Gets filled with the provided `data`
    ///    - Complete checkpoint: Returns unchanged
    ///
    /// 2. **If inserting at an existing height with a different hash:**
    ///    - The conflicting checkpoint and all above it are purged
    ///    - The new data is inserted at that height
    ///
    /// 3. **If inserting at a new height:**
    ///    - When `prev_blockhash` conflicts with the checkpoint below:
    ///      - That checkpoint is displaced (converted to placeholder)
    ///      - All checkpoints above are purged (they're now orphaned)
    ///    - The new data is inserted, potentially becoming the new tip
    ///
    /// ## Examples
    ///
    /// ```text
    /// // Inserting with conflicting prev_blockhash
    /// Before: 98 -> 99 -> 100 -> 101
    /// Insert: block_100_new with prev=different_99
    /// After:  98 -> 99_placeholder -> 100_new
    /// (Note: 101 was purged as it was orphaned)
    /// ```
    ///
    /// # Panics
    ///
    /// This panics if called with a genesis block that differs from that of `self`.
    #[must_use]
    pub fn insert(self, height: u32, data: D) -> Self {
        let hash = data.to_blockhash();
        let data_id = BlockId { hash, height };

        // Step 1: Split the chain into base (everything below height) and tail (height and above).
        // We collect the tail to re-insert after placing our new data.
        let mut base_opt = Some(self.clone());
        let mut tail = Vec::<(BlockId, D)>::new();
        for (cp_id, cp_data, prev) in self
            .iter()
            .filter(|cp| cp.height() != height)
            .take_while(|cp| cp.height() >= height + 1)
            .filter_map(|cp| Some((cp.block_id(), cp.data()?, cp.prev())))
        {
            base_opt = prev;
            tail.push((cp_id, cp_data));
        }
        let index = tail.partition_point(|(id, _)| id.height > height);
        tail.insert(index, (data_id, data));

        // Step 2: Rebuild the chain by pushing each element from tail onto base.
        // This process handles conflicts automatically through push's validation.
        while let Some((tail_id, tail_data)) = tail.pop() {
            let base = match base_opt {
                Some(base) => base,
                None => {
                    base_opt = Some(CheckPoint(Arc::new(CPInner {
                        block_id: tail_id,
                        data: Some(tail_data),
                        prev: None,
                    })));
                    continue;
                }
            };

            match base.push(tail_id.height, tail_data.clone()) {
                Ok(cp) => {
                    base_opt = Some(cp);
                    continue;
                }
                Err(cp) => {
                    if tail_id.height == height {
                        // Failed due to prev_blockhash conflict at height-1
                        if cp.height() + 1 == height {
                            // Displace the conflicting checkpoint; clear tail as those are now
                            // orphaned
                            base_opt = cp.prev();
                            tail.clear();
                            tail.push((tail_id, tail_data));
                            continue;
                        }

                        // Failed because height already exists
                        if cp.height() == height {
                            base_opt = cp.prev();
                            if cp.hash() != hash {
                                // Hash conflicts: purge everything above
                                tail.clear();
                            }
                            // If hash matches, keep tail (everything above remains valid)
                            tail.push((tail_id, tail_data));
                            continue;
                        }
                    }

                    if tail_id.height > height {
                        // Push failed for checkpoint above our insertion: this means the inserted
                        // data broke the chain continuity (orphaned checkpoints), so we stop here
                        base_opt = Some(cp);
                        break;
                    }

                    unreachable!(
                        "fail cannot be a result of pushing something that was part of `self`"
                    );
                }
            };
        }

        base_opt.expect("must have atleast one checkpoint")
    }

    /// Extends the chain forward by pushing a new checkpoint.
    ///
    /// This method is for extending the chain with new checkpoints at heights greater than
    /// the current tip. It maintains chain continuity by creating placeholders when necessary.
    ///
    /// ## Behavior
    ///
    /// - **Height validation**: Only accepts heights greater than the current tip
    /// - **Chain continuity**: For non-contiguous heights with `prev_blockhash`, creates a
    ///   placeholder at `height - 1` to maintain the chain link
    /// - **Conflict detection**: Fails if `prev_blockhash` doesn't match the expected parent
    /// - **Placeholder cleanup**: Removes any trailing placeholders before pushing
    ///
    /// ## Returns
    ///
    /// - `Ok(CheckPoint)`: Successfully extended chain with the new checkpoint as tip
    /// - `Err(self)`: Failed due to height ≤ current or `prev_blockhash` conflict
    ///
    /// ## Example
    ///
    /// ```text
    /// // Pushing non-contiguous height with prev_blockhash
    /// Before: 98 -> 99 -> 100
    /// Push:   block_105 with prev=block_104
    /// After:  98 -> 99 -> 100 -> 104_placeholder -> 105
    /// ```
    pub fn push(self, height: u32, data: D) -> Result<Self, Self> {
        // Reject if trying to push at or below current height - chain must grow forward
        if height <= self.height() {
            return Err(self);
        }

        let mut prev = Some(self.0.clone());

        // Remove any floating placeholder checkpoints.
        while let Some(inner) = prev {
            if inner.data.is_some() {
                prev = Some(inner);
                break;
            }
            prev = inner.prev.clone();
        }

        // Ensure we insert a placeholder if `prev.height` is not contiguous.
        if let (Some(prev_height), Some(prev_hash)) = (height.checked_sub(1), data.prev_blockhash())
        {
            prev = match prev {
                Some(inner) if inner.block_id.height == prev_height => {
                    // For contiguous height, ensure prev_blockhash does not conflict.
                    if inner.block_id.hash != prev_hash {
                        return Err(self);
                    }
                    // No placeholder needed as chain has non-empty checkpoint already.
                    Some(inner)
                }
                // Insert placeholder for non-contiguous height.
                prev => Some(Arc::new(CPInner {
                    block_id: BlockId {
                        height: prev_height,
                        hash: prev_hash,
                    },
                    data: None,
                    prev,
                })),
            };
        }

        Ok(Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data: Some(data),
            prev,
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
