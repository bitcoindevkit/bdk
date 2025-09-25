use alloc::sync::Arc;
use alloc::vec::Vec;
use bitcoin::{block::Header, BlockHash};
use core::fmt;
use core::ops::RangeBounds;

use crate::{BlockId, CheckPointEntry, CheckPointEntryIter};

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
/// Implementations of [`ToBlockHash`] must always return the block's consensus-defined hash. If
/// your type contains extra fields (timestamps, metadata, etc.), these must be ignored. For
/// example, [`BlockHash`] trivially returns itself, [`Header`] calls its `block_hash()`, and a
/// wrapper type around a [`Header`] should delegate to the header's hash rather than derive one
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

/// Trait that extracts a block time from [`CheckPoint`] `data`.
///
/// `data` types that contain a block time should implement this.
pub trait ToBlockTime {
    /// Returns the block time from the [`CheckPoint`] `data`.
    fn to_blocktime(&self) -> u32;
}

impl ToBlockTime for Header {
    fn to_blocktime(&self) -> u32 {
        self.time
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
    D: ToBlockHash + fmt::Debug + Clone,
{
    const MTP_BLOCK_COUNT: u32 = 11;

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

    /// Calculate the median time past (MTP) for this checkpoint.
    ///
    /// Uses 11 blocks (heights h-10 through h, where h is the current height) to compute the MTP
    /// for the current block. This is used in Bitcoin's consensus rules for time-based validations
    /// (BIP-0113).
    ///
    /// Note: This is a pseudo-median that doesn't average the two middle values.
    ///
    /// Returns `None` if the data type doesn't support block times or if any of the required
    /// 11 sequential blocks are missing.
    pub fn median_time_past(&self) -> Option<u32>
    where
        D: ToBlockTime,
    {
        let current_height = self.height();
        let earliest_height = current_height.saturating_sub(Self::MTP_BLOCK_COUNT - 1);

        let mut timestamps = (earliest_height..=current_height)
            .map(|height| {
                // Return `None` for missing blocks or missing block times
                let cp = self.get(height)?;
                let block_time = cp.data_ref().to_blocktime();
                Some(block_time)
            })
            .collect::<Option<Vec<u32>>>()?;
        timestamps.sort_unstable();

        // If there are more than 1 middle values, use the higher middle value.
        // This is mathematically incorrect, but this is the BIP-0113 specification.
        Some(timestamps[timestamps.len() / 2])
    }

    /// Construct from an iterator of block data.
    ///
    /// # Returns
    ///
    /// Returns the checkpoint chain tip on success.
    ///
    /// # Errors
    ///
    /// Returns `Err(None)` if `blocks` doesn't yield any data. If the blocks are not in ascending
    /// height order, or there are any `prev_blockhash` mismatches, then returns `Err(Some(..))`
    /// containing the last checkpoint that was successfully extended.
    pub fn from_blocks(blocks: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Option<Self>> {
        let mut blocks = blocks.into_iter();
        let (height, data) = blocks.next().ok_or(None)?;
        let mut cp = CheckPoint::new(height, data);
        cp = cp.extend(blocks).map_err(Some)?;

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
    /// If a checkpoint already exists at `height` with a matching hash, returns `self` unchanged.
    /// Otherwise, if the insertion conflicts — either with an existing checkpoint at `height` (by
    /// hash), or with the checkpoint at `height - 1` (via `data.prev_blockhash`) — every
    /// checkpoint at or above `height` is removed.
    ///
    /// # Panics
    ///
    /// Panics if the insertion would replace (or omit) the checkpoint at height 0 (a.k.a
    /// "genesis"). Although [`CheckPoint`] isn't structurally required to contain a genesis
    /// block, if one is present, it stays immutable and can't be replaced.
    #[must_use]
    pub fn insert(self, height: u32, data: D) -> Self {
        let mut cp = self.clone();
        let mut tail = vec![];

        // Traverse from tip to base, looking for where to insert.
        let base = loop {
            // Genesis (height 0) must remain immutable.
            if cp.height() == 0 {
                let implied_genesis = match height {
                    0 => Some(data.to_blockhash()),
                    1 => data.prev_blockhash(),
                    _ => None,
                };
                if let Some(hash) = implied_genesis {
                    assert_eq!(hash, cp.hash(), "inserted data implies different genesis");
                }
            }

            // Above insertion: collect for potential re-insertion later.
            // No need to check data.prev_blockhash here since that points below insertion. The
            // reverse relationship (cp.prev_blockhash vs data.hash) is validated during rebuild.
            if cp.height() > height {
                tail.push((cp.height(), cp.data()));

            // At insertion: determine whether we need to clear tail, or early return.
            } else if cp.height() == height {
                if cp.hash() == data.to_blockhash() {
                    return self;
                }
                tail.clear();

            // Displacement: data's prev_blockhash conflicts with this checkpoint,
            // so skip it and invalidate everything above.
            } else if cp.height() + 1 == height
                && data.prev_blockhash().is_some_and(|h| h != cp.hash())
            {
                tail.clear();

            // Below insertion: this is our base (since data's prev_blockhash does not conflict).
            } else if cp.height() < height {
                break Some(cp);
            }

            // Continue traversing down (if possible).
            match cp.prev() {
                Some(prev) => cp = prev,
                None => break None,
            }
        };

        tail.push((height, data));
        let tail = tail.into_iter().rev();

        // Reconstruct the chain: If a block above insertion has a prev_blockhash that doesn't match
        // the inserted data's hash, that block and everything above it are evicted.
        let (Ok(cp) | Err(cp)) = match base {
            Some(base_cp) => base_cp.extend(tail),
            None => CheckPoint::from_blocks(tail).map_err(|err| err.expect("tail is non-empty")),
        };
        cp
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if:
    /// * The block you are pushing on is not at a greater height that the one you are pushing on
    ///   to.
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

        let new_index = self.0.index + 1;

        // Skip pointers are added every CHECKPOINT_SKIP_INTERVAL (100) checkpoints
        // e.g., checkpoints at index 100, 200, 300, etc. have skip pointers
        let needs_skip_pointer =
            new_index >= CHECKPOINT_SKIP_INTERVAL && new_index % CHECKPOINT_SKIP_INTERVAL == 0;

        let skip = if needs_skip_pointer {
            // Skip pointer points back CHECKPOINT_SKIP_INTERVAL positions
            // e.g., checkpoint at index 200 points to checkpoint at index 100
            // We walk back CHECKPOINT_SKIP_INTERVAL - 1 steps since we start from self (index
            // new_index - 1)
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

        // At this point there should be exactly three strong references to the
        // genesis checkpoint:
        // 1. The variable `genesis`
        // 2. The chain `cp` through checkpoint 1's prev pointer
        // 3. Checkpoint at index 100's skip pointer (points to index 0)
        assert_eq!(
            Arc::strong_count(&genesis.0),
            3,
            "`cp`, `genesis`, and checkpoint 100's skip pointer should be the only strong references",
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
