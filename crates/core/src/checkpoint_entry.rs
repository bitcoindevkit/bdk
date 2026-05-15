//! Checkpoint entries for `prev_blockhash`-aware iteration.
//!
//! A [`CheckPoint`] chain may have gaps (non-contiguous heights). However, each checkpoint's
//! data can include a `prev_blockhash` that references the block one height below. This module
//! provides [`CheckPointEntry`], which represents either:
//!
//! - **Occupied**: A real checkpoint stored at this height.
//! - **Placeholder**: No checkpoint exists at this height, but the checkpoint above references it
//!   via `prev_blockhash`. The placeholder contains the implied [`BlockId`].
//!
//! Use [`CheckPoint::entry_iter`] to iterate over entries, which yields placeholders for any
//! gaps where `prev_blockhash` implies a block.

use bitcoin::BlockHash;

use crate::{BlockId, CheckPoint, ToBlockHash};

/// An entry yielded by [`CheckPointEntryIter`].
#[derive(Debug, Clone, PartialEq)]
pub enum CheckPointEntry<D> {
    /// A placeholder entry: there is no checkpoint stored at this height,
    /// but the checkpoint one height above links back here via its `prev_blockhash`.
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

    /// Returns `true` if this entry is a placeholder (inferred from `prev_blockhash`).
    pub fn is_placeholder(&self) -> bool {
        matches!(self, CheckPointEntry::Placeholder { .. })
    }

    /// The checkpoint that is the *source* of this entry.
    ///
    /// For an `Occupied` entry, this is the checkpoint itself.
    /// For a `Placeholder` entry, this is the checkpoint above that references this height.
    pub fn source_checkpoint(&self) -> CheckPoint<D> {
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
