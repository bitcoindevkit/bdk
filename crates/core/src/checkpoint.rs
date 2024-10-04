use core::ops::RangeBounds;

use alloc::sync::Arc;
use bitcoin::{block::Header, BlockHash};

use crate::BlockId;

/// A checkpoint is a node of a reference-counted linked list of [`BlockId`]s.
///
/// Checkpoints are cheaply cloneable and are useful to find the agreement point between two sparse
/// block chains.
#[derive(Debug)]
pub struct CheckPoint<B = BlockHash>(Arc<CPInner<B>>);

impl<B> Clone for CheckPoint<B> {
    fn clone(&self) -> Self {
        CheckPoint(Arc::clone(&self.0))
    }
}

/// The internal contents of [`CheckPoint`].
#[derive(Debug)]
struct CPInner<B> {
    /// Block data.
    block_id: BlockId,
    /// Data.
    data: B,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner<B>>>,
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

impl<B> PartialEq for CheckPoint<B>
where
    B: core::cmp::PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        let self_cps = self.iter().map(|cp| cp.0.block_id);
        let other_cps = other.iter().map(|cp| cp.0.block_id);
        self_cps.eq(other_cps)
    }
}

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
        since = "0.1.1",
        note = "Please use [`CheckPoint::blockhash_checkpoint_from_header`] instead. To create a CheckPoint<Header>, please use [`CheckPoint::from_data`]."
    )]
    pub fn from_header(header: &bitcoin::block::Header, height: u32) -> Self {
        CheckPoint::blockhash_checkpoint_from_header(header, height)
    }

    /// Construct a checkpoint from the given `header` and block `height`.
    ///
    /// If `header` is of the genesis block, the checkpoint won't have a [`prev`] node. Otherwise,
    /// we return a checkpoint linked with the previous block.
    ///
    /// [`prev`]: CheckPoint::prev
    pub fn blockhash_checkpoint_from_header(header: &bitcoin::block::Header, height: u32) -> Self {
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
            .push(this_block_id)
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
        let mut blocks = block_ids.into_iter();
        let block = blocks.next().ok_or(None)?;
        let mut acc = CheckPoint::new(block);
        for id in blocks {
            acc = acc.push(id).map_err(Some)?;
        }
        Ok(acc)
    }

    /// Extends the checkpoint linked list by a iterator of block ids.
    ///
    /// Returns an `Err(self)` if there is block which does not have a greater height than the
    /// previous one.
    pub fn extend(self, blockdata: impl IntoIterator<Item = BlockId>) -> Result<Self, Self> {
        self.extend_data(
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
    /// along with all block followin it. The returned chain will have a tip of the `block_id`
    /// passed in. Of course, if the `block_id` was already present then this just returns `self`.
    #[must_use]
    pub fn insert(self, block_id: BlockId) -> Self {
        self.insert_data(block_id.height, block_id.hash)
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the one you
    /// are pushing on to.
    pub fn push(self, block: BlockId) -> Result<Self, Self> {
        self.push_data(block.height, block.hash)
    }
}

impl<B> CheckPoint<B> {
    /// Get the `data` of the checkpoint.
    pub fn data(&self) -> &B {
        &self.0.data
    }

    /// Get the [`BlockId`] of the checkpoint.
    pub fn block_id(&self) -> BlockId {
        self.0.block_id
    }

    /// Get the `height` of the checkpoint.
    pub fn height(&self) -> u32 {
        self.0.block_id.height
    }

    /// Get the block hash of the checkpoint.
    pub fn hash(&self) -> BlockHash {
        self.0.block_id.hash
    }

    /// Get the previous checkpoint in the chain.
    pub fn prev(&self) -> Option<CheckPoint<B>> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter<B> {
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
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint<B>>
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
}

impl<B> CheckPoint<B>
where
    B: Copy + core::fmt::Debug + ToBlockHash,
{
    /// Construct a new base [`CheckPoint`] from given `height` and `data` at the front of a linked
    /// list.
    pub fn from_data(height: u32, data: B) -> Self {
        Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: None,
        }))
    }

    /// Extends the checkpoint linked list by a iterator containing `height` and `data`.
    ///
    /// Returns an `Err(self)` if there is block which does not have a greater height than the
    /// previous one.
    pub fn extend_data(self, blockdata: impl IntoIterator<Item = (u32, B)>) -> Result<Self, Self> {
        let mut curr = self.clone();
        for (height, data) in blockdata {
            curr = curr.push_data(height, data).map_err(|_| self.clone())?;
        }
        Ok(curr)
    }

    /// Inserts `data` at its `height` within the chain.
    ///
    /// The effect of `insert` depends on whether a `height` already exists. If it doesn't, the
    /// `data` we inserted and all pre-existing `data` at higher heights will be re-inserted after
    /// it. If the `height` already existed and has a conflicting block hash then it will be purged
    /// along with all block following it. The returned chain will have a tip with the `data`
    /// passed in. Of course, if the `data` was already present then this just returns `self`.
    #[must_use]
    pub fn insert_data(self, height: u32, data: B) -> Self {
        assert_ne!(height, 0, "cannot insert the genesis block");

        let mut cp = self.clone();
        let mut tail = vec![];
        let base = loop {
            if cp.height() == height {
                if cp.hash() == data.to_blockhash() {
                    return self;
                }
                // if we have a conflict we just return the inserted data because the tail is by
                // implication invalid.
                tail = vec![];
                break cp.prev().expect("can't be called on genesis block");
            }

            if cp.height() < height {
                break cp;
            }

            tail.push((cp.height(), *cp.data()));
            cp = cp.prev().expect("will break before genesis block");
        };

        base.extend_data(core::iter::once((height, data)).chain(tail.into_iter().rev()))
            .expect("tail is in order")
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if the block you are pushing on is not at a greater height that the one you
    /// are pushing on to.
    pub fn push_data(self, height: u32, data: B) -> Result<Self, Self> {
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

    /// This method tests for `self` and `other` to have equal internal pointers.
    pub fn eq_ptr(&self, other: &Self) -> bool {
        Arc::as_ptr(&self.0) == Arc::as_ptr(&other.0)
    }
}

/// Iterates over checkpoints backwards.
pub struct CheckPointIter<B = BlockHash> {
    current: Option<Arc<CPInner<B>>>,
}

impl<B> Iterator for CheckPointIter<B> {
    type Item = CheckPoint<B>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current.clone()?;
        self.current.clone_from(&current.prev);
        Some(CheckPoint(current))
    }
}

impl<B> IntoIterator for CheckPoint<B> {
    type Item = CheckPoint<B>;
    type IntoIter = CheckPointIter<B>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter {
            current: Some(self.0),
        }
    }
}
