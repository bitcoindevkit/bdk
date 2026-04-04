use crate::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Tracks block-height queries: which heights have been requested,
/// which are still pending, and the resolved data.
///
/// This is a helper for [`ChainQuery`](crate::ChainQuery) implementors that need to
/// track which block heights have been requested from the chain source and which
/// responses have been received. It deduplicates requests and stores resolved blocks.
///
/// # Usage
///
/// 1. Call [`request`](Self::request) with the heights you need — it returns only the heights that
///    haven't been requested or resolved yet.
/// 2. Feed responses back via [`resolve`](Self::resolve).
/// 3. Look up resolved blocks with [`get`](Self::get).
/// 4. Check for outstanding requests with [`unresolved`](Self::unresolved).
pub struct BlockQueries<B> {
    pending: BTreeSet<u32>,
    blocks: BTreeMap<u32, Option<B>>,
}

impl<B> BlockQueries<B> {
    /// Creates an empty `BlockQueries`.
    pub fn new() -> Self {
        Self {
            pending: BTreeSet::new(),
            blocks: BTreeMap::new(),
        }
    }

    /// Filters out already-resolved/pending heights, inserts new ones into pending,
    /// and returns the list of newly requested heights.
    pub fn request(&mut self, heights: impl Iterator<Item = u32>) -> Vec<u32> {
        heights
            .filter(|h| !self.blocks.contains_key(h))
            .filter(|h| self.pending.insert(*h))
            .collect()
    }

    /// Marks a height as resolved. Removes from pending and inserts into blocks.
    pub fn resolve(&mut self, height: u32, block: Option<B>) {
        let was_pending = self.pending.remove(&height);
        self.blocks.insert(height, block);
        debug_assert!(
            was_pending,
            "request must be previously unresolved to be resolved now"
        );
    }

    /// Iterates over heights that are still pending (unresolved).
    pub fn unresolved(&self) -> impl Iterator<Item = u32> + '_ {
        self.pending.iter().copied()
    }

    /// Looks up a resolved block by height.
    pub fn get(&self, height: u32) -> Option<&Option<B>> {
        self.blocks.get(&height)
    }
}

impl<B> Default for BlockQueries<B> {
    fn default() -> Self {
        Self::new()
    }
}
