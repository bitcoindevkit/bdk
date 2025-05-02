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
pub struct CheckPoint<H = BlockHash>(Arc<CPInner<H>>);

impl<H> Clone for CheckPoint<H> {
    fn clone(&self) -> Self {
        CheckPoint(Arc::clone(&self.0))
    }
}

/// The internal contents of [`CheckPoint`].
#[derive(Debug)]
struct CPInner<H> {
    /// Block id
    block_id: BlockId,
    /// Data.
    data: H,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner<H>>>,
}

/// When a `CPInner` is dropped we need to go back down the chain and manually remove any
/// no-longer referenced checkpoints. Letting the default rust dropping mechanism handle this
/// leads to recursive logic and stack overflows
///
/// https://github.com/bitcoindevkit/bdk/issues/1634
impl<H> Drop for CPInner<H> {
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

impl<H> PartialEq for CheckPoint<H> {
    fn eq(&self, other: &Self) -> bool {
        let self_cps = self.iter().map(|cp| cp.block_id());
        let other_cps = other.iter().map(|cp| cp.block_id());
        self_cps.eq(other_cps)
    }
}

// Methods for `CheckPoint<BlockHash>`
impl CheckPoint<BlockHash> {
    /// Construct a new base [`CheckPoint`] at the front of a linked list.
    pub fn new(block: BlockId) -> Self {
        CheckPoint::from_data(block.height, block.hash)
    }

    /// Construct a checkpoint from the given `header` and block `height`.
    ///
    /// If `header` is of the genesis block, the checkpoint won't have a `prev` node. Otherwise,
    /// we return a checkpoint linked with the previous block.
    #[deprecated(
        since = "0.5.0",
        note = "Use `blockhash_checkpoint_from_header` instead."
    )]
    pub fn from_header(header: &Header, height: u32) -> Self {
        CheckPoint::blockhash_checkpoint_from_header(header, height)
    }

    /// Construct a checkpoint from the given `header` and block `height`.
    ///
    /// If `header` is of the genesis block, the checkpoint won't have a [`prev`] node. Otherwise,
    /// we return a checkpoint linked with the previous block.
    ///
    /// [`prev`]: CheckPoint::prev
    pub fn blockhash_checkpoint_from_header(header: &Header, height: u32) -> Self {
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
            .push_block_id(this_block_id)
            .expect("must construct checkpoint")
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
        Self::from_block_data(block_ids.into_iter().map(|b| (b.height, b.hash)))
    }

    /// Extends the checkpoint linked list by a iterator of block ids.
    ///
    /// Returns an `Err(self)` if there is block which does not have a greater height than the
    /// previous one.
    pub fn extend_block_ids(
        self,
        blockdata: impl IntoIterator<Item = BlockId>,
    ) -> Result<Self, Self> {
        self.extend(
            blockdata
                .into_iter()
                .map(|block| (block.height, block.hash)),
        )
    }

    /// Inserts `block_id` at its height within the chain.
    ///
    /// The effect of `insert` depends on whether a height already exists. If it doesn't the
    /// `block_id` we inserted and all pre-existing blocks higher than it will be re-inserted after
    /// it. If the height already existed and has a conflicting block hash then it will be purged
    /// along with all blocks following it. The returned chain will have a tip of the `block_id`
    /// passed in. Of course, if the `block_id` was already present then this just returns `self`.
    #[must_use]
    pub fn insert_block_id(self, block_id: BlockId) -> Self {
        self.insert(block_id.height, block_id.hash)
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the one you
    /// are pushing on to.
    pub fn push_block_id(self, block: BlockId) -> Result<Self, Self> {
        self.push(block.height, block.hash)
    }
}

// Methods for any `H`
impl<H> CheckPoint<H> {
    /// Get a reference of the `data` of the checkpoint.
    pub fn data_ref(&self) -> &H {
        &self.0.data
    }

    /// Get the `data` of a the checkpoint.
    pub fn data(&self) -> H
    where
        H: Clone,
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
    pub fn prev(&self) -> Option<CheckPoint<H>> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter<H> {
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
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint<H>>
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

    /// This method tests for `self` and `other` to have equal internal pointers.
    pub fn eq_ptr(&self, other: &Self) -> bool {
        Arc::as_ptr(&self.0) == Arc::as_ptr(&other.0)
    }
}

// Methods where `H: ToBlockHash`
impl<H> CheckPoint<H>
where
    H: ToBlockHash + fmt::Debug + Copy,
{
    /// Construct a new base [`CheckPoint`] from given `height` and `data` at the front of a linked
    /// list.
    pub fn from_data(height: u32, data: H) -> Self {
        Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: None,
        }))
    }

    /// New from an iterator of block data.
    ///
    /// Returns `Err(None)` if `blocks` doesn't yield any data. If the blocks are not in ascending
    /// height order, then returns the last [`CheckPoint`] that would have been extended.
    pub fn from_block_data(
        blocks: impl IntoIterator<Item = (u32, H)>,
    ) -> Result<Self, Option<Self>> {
        let mut blocks = blocks.into_iter();
        let (height, data) = blocks.next().ok_or(None)?;
        let mut cp = CheckPoint::from_data(height, data);
        cp = cp.extend(blocks)?;

        Ok(cp)
    }

    /// Extends the checkpoint linked list by a iterator containing `height` and `data`.
    ///
    /// Returns an `Err(self)` if there is block which does not have a greater height than the
    /// previous one.
    pub fn extend(self, blockdata: impl IntoIterator<Item = (u32, H)>) -> Result<Self, Self> {
        let mut cp = self.clone();
        for (height, data) in blockdata {
            cp = cp.push(height, data)?;
        }
        Ok(cp)
    }

    /// Inserts `data` at its `height` within the chain.
    ///
    /// The effect of `insert` depends on whether a height already exists. If it doesn't the
    /// `block_id` we inserted and all pre-existing blocks higher than it will be re-inserted after
    /// it. If the height already existed and has a conflicting block hash then it will be purged
    /// along with all block following it. The returned chain will have a tip of the `block_id`
    /// passed in. Of course, if the `block_id` was already present then this just returns `self`.
    ///
    /// # Panics
    ///
    /// This panics if called with a genesis block that differs from that of `self`.
    #[must_use]
    pub fn insert(self, height: u32, data: H) -> Self {
        let mut cp = self.clone();
        let mut tail = vec![];
        let base = loop {
            if cp.height() == height {
                if cp.hash() == data.to_blockhash() {
                    return self;
                }
                assert_ne!(cp.height(), 0, "cannot replace genesis block");
                // if we have a conflict we just return the inserted block because the tail is by
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
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the one you
    /// are pushing on to.
    pub fn push(self, height: u32, data: H) -> Result<Self, Self> {
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
pub struct CheckPointIter<H> {
    current: Option<Arc<CPInner<H>>>,
}

impl<H> Iterator for CheckPointIter<H> {
    type Item = CheckPoint<H>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.clone()?;
        self.current.clone_from(&current.prev);
        Some(CheckPoint(current))
    }
}

impl<H> IntoIterator for CheckPoint<H> {
    type Item = CheckPoint<H>;
    type IntoIter = CheckPointIter<H>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter {
            current: Some(self.0),
        }
    }
}
