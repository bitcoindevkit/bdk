use crate::collections::HashSet;
use core::marker::PhantomData;

use alloc::{collections::VecDeque, vec::Vec};
use bitcoin::BlockHash;

use crate::BlockId;

/// Represents a service that tracks the blockchain.
///
/// The main method is [`is_block_in_chain`] which determines whether a given block of [`BlockId`]
/// is an ancestor of another "static block".
///
/// [`is_block_in_chain`]: Self::is_block_in_chain
pub trait ChainOracle {
    /// Error type.
    type Error: core::fmt::Debug;

    /// Determines whether `block` of [`BlockId`] exists as an ancestor of `static_block`.
    ///
    /// If `None` is returned, it means the implementation cannot determine whether `block` exists.
    fn is_block_in_chain(
        &self,
        block: BlockId,
        static_block: BlockId,
    ) -> Result<Option<bool>, Self::Error>;
}

/// A cache structure increases the performance of getting chain data.
///
/// A simple FIFO cache replacement policy is used. Something more efficient and advanced can be
/// implemented later.
#[derive(Debug, Default)]
pub struct CacheBackend<C> {
    cache: HashSet<(BlockHash, BlockHash)>,
    fifo: VecDeque<(BlockHash, BlockHash)>,
    marker: PhantomData<C>,
}

impl<C> CacheBackend<C> {
    /// Get the number of elements in the cache.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// Prunes the cache to reach the `max_size` target.
    ///
    /// Returns pruned elements.
    pub fn prune(&mut self, max_size: usize) -> Vec<(BlockHash, BlockHash)> {
        let prune_count = self.cache.len().saturating_sub(max_size);
        (0..prune_count)
            .filter_map(|_| self.fifo.pop_front())
            .filter(|k| self.cache.remove(k))
            .collect()
    }

    pub fn contains(&self, static_block: BlockId, block: BlockId) -> bool {
        if static_block.height < block.height
            || static_block.height == block.height && static_block.hash != block.hash
        {
            return false;
        }

        self.cache.contains(&(static_block.hash, block.hash))
    }

    pub fn insert(&mut self, static_block: BlockId, block: BlockId) -> bool {
        let cache_key = (static_block.hash, block.hash);

        if self.cache.insert(cache_key) {
            self.fifo.push_back(cache_key);
            true
        } else {
            false
        }
    }
}
