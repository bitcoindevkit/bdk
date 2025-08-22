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

    /// Returns `None` if the type has no knowledge of the previous blockhash.
    fn prev_blockhash(&self) -> Option<BlockHash>;
}

impl ToBlockHash for BlockHash {
    fn to_blockhash(&self) -> BlockHash {
        *self
    }

    fn prev_blockhash(&self) -> Option<BlockHash> {
        None
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

impl<D: ToBlockHash> PartialEq for CheckPoint<D> {
    fn eq(&self, other: &Self) -> bool {
        let self_cps = self
            .iter()
            .filter_map(|item| item.checkpoint())
            .map(|cp| cp.block_id());
        let other_cps = other
            .iter()
            .filter_map(|item| item.checkpoint())
            .map(|cp| cp.block_id());
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
}

impl<D: ToBlockHash> CheckPoint<D> {
    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter<D> {
        self.clone().into_iter()
    }

    /// Get checkpoint at `height`.
    ///
    /// Returns `None` if checkpoint at `height` does not exist.
    pub fn get(&self, height: u32) -> Option<CheckPoint<D>> {
        self.entry(height).and_then(|entry| entry.checkpoint())
    }

    /// TODO: Docs.
    pub fn entry(&self, height: u32) -> Option<CheckPointEntry<D>> {
        self.range(height..=height).next()
    }

    /// Iterate checkpoints over a height range.
    ///
    /// Note that we always iterate checkpoints in reverse height order (iteration starts at tip
    /// height).
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPointEntry<D>>
    where
        R: RangeBounds<u32>,
    {
        let start_bound = range.start_bound().cloned();
        let end_bound = range.end_bound().cloned();
        self.iter()
            .skip_while(move |cp| match end_bound {
                core::ops::Bound::Included(inc_bound) => cp.block_id().height > inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp.block_id().height >= exc_bound,
                core::ops::Bound::Unbounded => false,
            })
            .take_while(move |cp| match start_bound {
                core::ops::Bound::Included(inc_bound) => cp.block_id().height >= inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp.block_id().height > exc_bound,
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
        }))
    }

    /// Construct from an iterator of block data.
    ///
    /// Returns `Err(None)` if `blocks` doesn't yield any data. If the blocks are not in ascending
    /// height order, then returns an `Err(..)` containing the last checkpoint that would have been
    /// extended.
    pub fn from_blocks(blocks: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Option<Self>> {
        // TODO: Also check if `prev_blockhash`-es match up.
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
        // TODO: Also check if `prev_blockhash`-es match up.
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
        // TODO: Also check if `prev_blockhash`-es match up.
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

        base.extend(core::iter::once((height, data)).chain(tail.into_iter().rev()))
            .expect("tail is in order")
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height than the
    /// one you are pushing on to.
    pub fn push(self, height: u32, data: D) -> Result<Self, Self> {
        // TODO: Also check if `prev_blockhash`-es match up.
        if self.height() < height {
            Ok(Self(Arc::new(CPInner {
                block_id: BlockId {
                    height,
                    hash: data.to_blockhash(),
                },
                data,
                prev: Some(self.0),
            })))
        } else {
            Err(self)
        }
    }
}

/// Iterates over checkpoints backwards.
pub struct CheckPointIter<D> {
    current: Option<CheckPointEntry<D>>,
}

/// An entry yielded by [`CheckPointIter`].
///
/// Each entry corresponds to a specific chain height. It is either:
/// - a real checkpoint stored at that height, or
/// - a back‑reference indicating that the checkpoint one height *above* links back to this height
///   via its `prev_blockhash`.
///
/// Emitting `Backref` entries means iteration won’t silently skip heights: you can still see which
/// block ID connects the gap even when no checkpoint was recorded at that exact height. Use
/// [`CheckPointEntry::backing_checkpoint`] to recover the checkpoint that backs this height in all
/// cases.
pub enum CheckPointEntry<D> {
    /// A real checkpoint recorded at this exact height.
    AtHeight(CheckPoint<D>),

    /// A "gap" entry: there is no checkpoint stored at this height, but the checkpoint one height
    /// above links back here via its `prev_blockhash`.
    Backref {
        /// The block ID at *this* height (the one being linked to by the
        /// checkpoint above).
        block_id: BlockId,

        /// The checkpoint one height *above* that links back to `block_id`.
        ///
        /// This is the checkpoint that ultimately backs this entry and will be returned by
        /// [`CheckPointEntry::nearest_checkpoint`].
        linking_checkpoint: CheckPoint<D>,
    },
}

impl<D> CheckPointEntry<D> {
    /// Returns the checkpoint recorded *at this exact height*, if any.
    ///
    /// - For [`CheckPointEntry::AtHeight`], this returns `Some(checkpoint)`.
    /// - For [`CheckPointEntry::Backref`], this returns `None`, because no checkpoint was
    /// explicitly stored at this height.
    ///
    /// Use [`CheckPointEntry::backing_checkpoint`] if you always need a checkpoint regardless of
    /// whether one exists at this height.
    pub fn checkpoint(&self) -> Option<CheckPoint<D>> {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => Some(checkpoint.clone()),
            CheckPointEntry::Backref { .. } => None,
        }
    }

    /// Returns the checkpoint that *backs* this entry.
    ///
    /// - For [`CheckPointEntry::AtHeight`], this is the checkpoint recorded at the current height.
    /// - For [`CheckPointEntry::Backref`], this is the checkpoint one height *above*, whose
    ///   `prev_blockhash` links back to this height.
    ///
    /// Unlike [`CheckPointEntry::checkpoint`], this method always returns a checkpoint regardless
    /// of whether one exists at the exact height.
    pub fn backing_checkpoint(&self) -> CheckPoint<D> {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => checkpoint.clone(),
            CheckPointEntry::Backref {
                linking_checkpoint: checkpoint,
                ..
            } => checkpoint.clone(),
        }
    }

    /// Returns the checkpoint at or below this entry’s height.
    ///
    /// - For [`CheckPointEntry::AtHeight`], this returns the checkpoint stored at the current
    ///   height.
    /// - For [`CheckPointEntry::Backref`], this returns the predecessor of the `linking_checkpoint`
    ///   (i.e. the closest checkpoint at a lower height).
    ///
    /// This is equivalent to taking the “floor” of the current height over the linked list of
    /// checkpoints. Returns `None` only if no lower checkpoint exists (e.g. iteration has reached
    /// genesis).
    pub fn floor_checkpoint(&self) -> Option<CheckPoint<D>> {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => Some(checkpoint.clone()),
            CheckPointEntry::Backref {
                linking_checkpoint, ..
            } => linking_checkpoint.prev(),
        }
    }

    /// Returns a reference to the data recorded *at this exact height*, if any.
    ///
    /// - For [`CheckPointEntry::AtHeight`], this returns `Some(&D)`.
    /// - For [`CheckPointEntry::Backref`], this returns `None`, because no checkpoint was
    ///   explicitly stored at this height.
    ///
    /// Use [`CheckPoint::data`] if you would like to returns non-referenced data.
    pub fn data_ref(&self) -> Option<&D> {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => Some(checkpoint.data_ref()),
            CheckPointEntry::Backref { .. } => None,
        }
    }

    /// Returns the data recorded *at this exact height*, if any.
    ///
    /// - For [`CheckPointEntry::AtHeight`], this returns `Some(&D)`.
    /// - For [`CheckPointEntry::Backref`], this returns `None`, because no checkpoint was
    ///   explicitly stored at this height.
    ///
    /// Use [`CheckPoint::data_ref`] if you would like to return a reference to the data instead.
    pub fn data(&self) -> Option<D>
    where
        D: Clone,
    {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => Some(checkpoint.data()),
            CheckPointEntry::Backref { .. } => None,
        }
    }
}

impl<D: ToBlockHash> CheckPointEntry<D> {
    /// The block ID of this entry.
    pub fn block_id(&self) -> BlockId {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => checkpoint.block_id(),
            CheckPointEntry::Backref { block_id: prev, .. } => *prev,
        }
    }

    /// The blockhash of this entry.
    pub fn hash(&self) -> BlockHash {
        self.block_id().hash
    }

    /// The block height of this entry.
    pub fn height(&self) -> u32 {
        self.block_id().height
    }

    fn try_prev(&self) -> Option<Self> {
        match self {
            CheckPointEntry::AtHeight(checkpoint) => {
                // Previous checkpoint (referenced by this checkpoint).
                let prev_cp_opt = checkpoint.prev();
                // Previous block's hash (if available).
                let backref_hash_opt = checkpoint.data_ref().prev_blockhash();

                match (prev_cp_opt, backref_hash_opt) {
                    (Some(prev_cp), Some(backref_hash)) => {
                        let backref_height = checkpoint.height().checked_sub(1).expect(
                            "there must not be a previous checkpoint below the height of 0",
                        );
                        if prev_cp.height() + 1 == checkpoint.height() {
                            Some(Self::AtHeight(prev_cp))
                        } else {
                            Some(Self::Backref {
                                block_id: BlockId {
                                    height: backref_height,
                                    hash: backref_hash,
                                },
                                linking_checkpoint: checkpoint.clone(),
                            })
                        }
                    }
                    (Some(prev_cp), None) => Some(Self::AtHeight(prev_cp)),
                    (None, backref_hash) => Some(Self::Backref {
                        block_id: BlockId {
                            height: checkpoint.height().checked_sub(1)?,
                            hash: backref_hash?,
                        },
                        linking_checkpoint: checkpoint.clone(),
                    }),
                }
            }
            CheckPointEntry::Backref {
                linking_checkpoint: checkpoint,
                ..
            } => checkpoint.prev().map(Self::AtHeight),
        }
    }
}

impl<D: ToBlockHash> Iterator for CheckPointIter<D> {
    type Item = CheckPointEntry<D>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.current.take()?;
        self.current = item.try_prev();
        Some(item)
    }
}

impl<D: ToBlockHash> IntoIterator for CheckPoint<D> {
    type Item = CheckPointEntry<D>;
    type IntoIter = CheckPointIter<D>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter {
            current: Some(CheckPointEntry::AtHeight(self)),
        }
    }
}
