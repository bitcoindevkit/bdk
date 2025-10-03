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
    /// The effect of `insert` depends on whether a height already exists. If it doesn't, the data
    /// we inserted and all pre-existing entries higher than it will be re-inserted after it. If the
    /// height already existed and has a conflicting block hash then it will be purged along with
    /// all entries following it. If the existing checkpoint at height is a placeholder where
    /// `data: None` with the same hash, then the `data` is inserted to make a complete checkpoint.
    /// The returned chain will have a tip of the data passed in. If the data was already present
    /// then this just returns `self`.
    ///
    /// When inserting data with a `prev_blockhash` that conflicts with existing checkpoints,
    /// those checkpoints will be displaced and replaced with placeholders. When inserting data
    /// whose block hash conflicts with the `prev_blockhash` of higher checkpoints, those higher
    /// checkpoints will be purged.
    ///
    /// # Panics
    ///
    /// This panics if called with a genesis block that differs from that of `self`.
    #[must_use]
    pub fn insert(self, height: u32, data: D) -> Self {
        let mut cp = self.clone();
        let mut tail: Vec<(u32, D)> = vec![];
        let mut base = loop {
            if cp.height() == height {
                let same_hash = cp.hash() == data.to_blockhash();
                if same_hash {
                    if cp.data().is_some() {
                        return self;
                    } else {
                        // If `CheckPoint` is a placeholder, return previous `CheckPoint`.
                        break cp.prev().expect("can't be called on genesis block");
                    }
                } else {
                    assert_ne!(cp.height(), 0, "cannot replace genesis block");
                    // If we have a conflict we just return the inserted data because the tail is by
                    // implication invalid.
                    tail = vec![];
                    break cp.prev().expect("can't be called on genesis block");
                }
            }

            if cp.height() < height {
                break cp;
            }

            if let Some(d) = cp.data() {
                tail.push((cp.height(), d));
            }
            cp = cp.prev().expect("will break before genesis block");
        };

        if let Some(prev_hash) = data.prev_blockhash() {
            // Check if the new data's `prev_blockhash` conflicts with the checkpoint at height - 1.
            if let Some(lower_cp) = base.get(height.saturating_sub(1)) {
                // New data's `prev_blockhash` conflicts with existing checkpoint, so we displace
                // the existing checkpoint and create a placeholder.
                if lower_cp.hash() != prev_hash {
                    // Find the base to link to at height - 2 or lower with actual data.
                    // We skip placeholders because when we displace a checkpoint, we can't ensure
                    // that placeholders below it still maintain proper chain continuity.
                    let link_base = if height > 1 {
                        base.find_data(height - 2)
                    } else {
                        None
                    };

                    // Create a new placeholder at height - 1 with the required `prev_blockhash`.
                    base = Self(Arc::new(CPInner {
                        block_id: BlockId {
                            height: height - 1,
                            hash: prev_hash,
                        },
                        data: None,
                        prev: link_base.map(|cb| cb.0),
                    }));
                }
            } else {
                // No checkpoint at height - 1, but we may need to create a placeholder.
                if height > 0 {
                    base = Self(Arc::new(CPInner {
                        block_id: BlockId {
                            height: height - 1,
                            hash: prev_hash,
                        },
                        data: None,
                        prev: base.0.prev.clone(),
                    }));
                }
            }
        }

        // Check for conflicts with higher checkpoints and purge if necessary.
        let mut filtered_tail = Vec::new();
        for (tail_height, tail_data) in tail.into_iter().rev() {
            // Check if this tail entry's `prev_blockhash` conflicts with our new data's blockhash.
            if let Some(tail_prev_hash) = tail_data.prev_blockhash() {
                // Conflict detected, so purge this and all tail entries.
                if tail_prev_hash != data.to_blockhash() {
                    break;
                }
            }
            filtered_tail.push((tail_height, tail_data));
        }

        base.extend(core::iter::once((height, data)).chain(filtered_tail))
            .expect("tail is in order")
    }

    /// Extends the chain by pushing a new checkpoint.
    ///
    /// Returns `Err(self)` if the height is not greater than the current height, or if the data's
    /// `prev_blockhash` conflicts with `self`.
    ///
    /// Creates a placeholder at height - 1 if the height is non-contiguous and
    /// `data.prev_blockhash()` is available.
    pub fn push(mut self, height: u32, data: D) -> Result<Self, Self> {
        // Reject if trying to push at or below current height - chain must grow forward
        if height <= self.height() {
            return Err(self);
        }

        if let Some(prev_hash) = data.prev_blockhash() {
            if height == self.height() + 1 {
                // For contiguous height, validate that prev_blockhash matches our hash
                // to ensure chain continuity
                if self.hash() != prev_hash {
                    return Err(self);
                }
            } else {
                // For non-contiguous heights, create placeholder to maintain chain linkage
                // This allows sparse chains while preserving block relationships
                self = CheckPoint(Arc::new(CPInner {
                    block_id: BlockId {
                        height: height
                            .checked_sub(1)
                            .expect("height has previous blocks so must be greater than 0"),
                        hash: prev_hash,
                    },
                    data: None,
                    prev: Some(self.0),
                }));
            }
        }

        Ok(Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data: Some(data),
            prev: Some(self.0),
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
