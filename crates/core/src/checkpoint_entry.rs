use core::ops::RangeBounds;

use bitcoin::BlockHash;

use crate::{BlockId, CheckPoint, ToBlockHash};

/// An entry yielded by [`CheckPointIter`].
#[derive(Debug, Clone)]
pub enum CheckPointEntry<D> {
    /// A placeholder entry: there is no checkpoint stored st this height,
    /// but the checkpoint one height above links back here via it's `prev_blockhash`.
    Placeholder {
        /// The block ID at *this* height.
        block_id: BlockId,
        /// The checkpoint one height *above* that links back to `block_id`.
        checkpoint_above: CheckPoint<D>,
    },
    /// A real checkpoint recorded at this height.
    Occupied(CheckPoint<D>),
}

impl<D> CheckPointEntry<D> {
    /// The checkpoint at this height (if any).
    pub fn checkpoint(&self) -> Option<CheckPoint<D>> {
        match self {
            CheckPointEntry::Placeholder { .. } => None,
            CheckPointEntry::Occupied(checkpoint) => Some(checkpoint.clone()),
        }
    }

    /// The checkpoint that *backs* this entry.
    pub fn backing_checkpoint(&self) -> CheckPoint<D> {
        match self {
            CheckPointEntry::Placeholder {
                checkpoint_above, ..
            } => checkpoint_above.clone(),
            CheckPointEntry::Occupied(checkpoint) => checkpoint.clone(),
        }
    }

    /// Returns the checkpoint at or below this entry's height.
    pub fn floor_checkpoint(&self) -> Option<CheckPoint<D>> {
        match self {
            CheckPointEntry::Placeholder {
                checkpoint_above: linking_checkpoint,
                ..
            } => linking_checkpoint.prev(),
            CheckPointEntry::Occupied(checkpoint) => Some(checkpoint.clone()),
        }
    }

    /// Returns a reference to the data recorded at this exact height (if any).
    pub fn data_ref(&self) -> Option<&D> {
        match self {
            CheckPointEntry::Placeholder { .. } => None,
            CheckPointEntry::Occupied(checkpoint) => Some(checkpoint.data_ref()),
        }
    }

    /// Returns the data recorded at this exact height (if any).
    pub fn data(&self) -> Option<D>
    where
        D: Clone,
    {
        self.data_ref().cloned()
    }
}

impl<D: ToBlockHash> CheckPointEntry<D> {
    /// The block ID of this entry.
    pub fn block_id(&self) -> BlockId {
        match self {
            CheckPointEntry::Placeholder { block_id, .. } => *block_id,
            CheckPointEntry::Occupied(checkpoint) => checkpoint.block_id(),
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

    /// Get the previous entry in the chain.
    pub fn prev(&self) -> Option<Self> {
        let checkpoint = match self {
            Self::Placeholder {
                checkpoint_above, ..
            } => {
                return checkpoint_above.prev().map(Self::Occupied);
            }
            Self::Occupied(checkpoint) => checkpoint,
        };

        let prev_height = checkpoint.height().checked_sub(1)?;

        let prev_blockhash = match checkpoint.data_ref().prev_blockhash() {
            Some(blockhash) => blockhash,
            None => return checkpoint.prev().map(Self::Occupied),
        };

        if let Some(prev_checkpoint) = checkpoint.prev() {
            if prev_checkpoint.height() == prev_height {
                return Some(Self::Occupied(prev_checkpoint));
            }
        }

        Some(Self::Placeholder {
            block_id: BlockId {
                height: prev_height,
                hash: prev_blockhash,
            },
            checkpoint_above: checkpoint.clone(),
        })
    }

    /// Iterate over checkpoint entries backwards.
    pub fn iter(&self) -> CheckPointEntryIter<D>
    where
        D: Clone,
    {
        self.clone().into_iter()
    }

    /// Get checkpoint entry at `height`.
    ///
    /// Returns `None` if checkpoint at `height` does not exist.
    pub fn get(&self, height: u32) -> Option<Self>
    where
        D: Clone,
    {
        self.range(height..=height).next()
    }

    /// Iterate checkpoints over a height range.
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPointEntry<D>>
    where
        D: Clone,
        R: RangeBounds<u32>,
    {
        let start_bound = range.start_bound().cloned();
        let end_bound = range.end_bound().cloned();
        self.iter()
            .skip_while(move |cp_entry| match end_bound {
                core::ops::Bound::Included(inc_bound) => cp_entry.height() > inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp_entry.height() >= exc_bound,
                core::ops::Bound::Unbounded => false,
            })
            .take_while(move |cp_entry| match start_bound {
                core::ops::Bound::Included(inc_bound) => cp_entry.height() >= inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp_entry.height() > exc_bound,
                core::ops::Bound::Unbounded => true,
            })
    }

    /// Returns the entry at `height` if one exists, otherwise the nearest checkpoint at a lower
    /// height.
    pub fn floor_at(&self, height: u32) -> Option<Self>
    where
        D: Clone,
    {
        self.range(..=height).next()
    }

    /// Returns the entry located a number of heights below this one.
    pub fn floor_below(&self, offset: u32) -> Option<Self>
    where D: Clone
    {
        self.floor_at(self.height().checked_sub(offset)?)
    }
}

/// Iterates over checkpoint entries backwards.
pub struct CheckPointEntryIter<D> {
    next: Option<CheckPointEntry<D>>,
}

impl<D: ToBlockHash> Iterator for CheckPointEntryIter<D> {
    type Item = CheckPointEntry<D>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.next.take()?;
        self.next = item.prev();
        Some(item)
    }
}

impl<D: ToBlockHash> IntoIterator for CheckPointEntry<D> {
    type Item = CheckPointEntry<D>;
    type IntoIter = CheckPointEntryIter<D>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointEntryIter { next: Some(self) }
    }
}
