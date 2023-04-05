use core::{convert::Infallible, marker::PhantomData};

use alloc::collections::BTreeMap;
use bitcoin::BlockHash;

use crate::BlockId;

/// Represents a service that tracks the best chain history.
/// TODO: How do we ensure the chain oracle is consistent across a single call?
/// * We need to somehow lock the data! What if the ChainOracle is remote?
/// * Get tip method! And check the tip still exists at the end! And every internal call
///   does not go beyond the initial tip.
pub trait ChainOracle {
    /// Error type.
    type Error: core::fmt::Debug;

    /// Get the height and hash of the tip in the best chain.
    fn get_tip_in_best_chain(&self) -> Result<Option<BlockId>, Self::Error>;

    /// Returns the block hash (if any) of the given `height`.
    fn get_block_in_best_chain(&self, height: u32) -> Result<Option<BlockHash>, Self::Error>;

    /// Determines whether the block of [`BlockId`] exists in the best chain.
    fn is_block_in_best_chain(&self, block_id: BlockId) -> Result<bool, Self::Error> {
        Ok(matches!(self.get_block_in_best_chain(block_id.height)?, Some(h) if h == block_id.hash))
    }
}

// [TODO] We need stuff for smart pointers. Maybe? How does rust lib do this?
// Box<dyn ChainOracle>, Arc<dyn ChainOracle> ????? I will figure it out
impl<C: ChainOracle> ChainOracle for &C {
    type Error = C::Error;

    fn get_tip_in_best_chain(&self) -> Result<Option<BlockId>, Self::Error> {
        <C as ChainOracle>::get_tip_in_best_chain(self)
    }

    fn get_block_in_best_chain(&self, height: u32) -> Result<Option<BlockHash>, Self::Error> {
        <C as ChainOracle>::get_block_in_best_chain(self, height)
    }

    fn is_block_in_best_chain(&self, block_id: BlockId) -> Result<bool, Self::Error> {
        <C as ChainOracle>::is_block_in_best_chain(self, block_id)
    }
}

/// This structure increases the performance of getting chain data.
#[derive(Debug)]
pub struct Cache<C> {
    assume_final_depth: u32,
    tip_height: u32,
    cache: BTreeMap<u32, BlockHash>,
    marker: PhantomData<C>,
}

impl<C> Cache<C> {
    /// Creates a new [`Cache`].
    ///
    /// `assume_final_depth` represents the minimum number of blocks above the block in question
    /// when we can assume the block is final (reorgs cannot happen). I.e. a value of 0 means the
    /// tip is assumed to be final. The cache only caches blocks that are assumed to be final.
    pub fn new(assume_final_depth: u32) -> Self {
        Self {
            assume_final_depth,
            tip_height: 0,
            cache: Default::default(),
            marker: Default::default(),
        }
    }
}

impl<C: ChainOracle> Cache<C> {
    /// This is the topmost (highest) block height that we assume as final (no reorgs possible).
    ///
    /// Blocks higher than this height are not cached.
    pub fn assume_final_height(&self) -> u32 {
        self.tip_height.saturating_sub(self.assume_final_depth)
    }

    /// Update the `tip_height` with the [`ChainOracle`]'s tip.
    ///
    /// `tip_height` is used with `assume_final_depth` to determine whether we should cache a
    /// certain block height (`tip_height` - `assume_final_depth`).
    pub fn try_update_tip_height(&mut self, chain: C) -> Result<(), C::Error> {
        let tip = chain.get_tip_in_best_chain()?;
        if let Some(BlockId { height, .. }) = tip {
            self.tip_height = height;
        }
        Ok(())
    }

    /// Get a block from the cache with the [`ChainOracle`] as fallback.
    ///
    /// If the block does not exist in cache, the logic fallbacks to fetching from the internal
    /// [`ChainOracle`]. If the block is at or below the "assume final height", we will also store
    /// the missing block in the cache.
    pub fn try_get_block(&mut self, chain: C, height: u32) -> Result<Option<BlockHash>, C::Error> {
        if let Some(&hash) = self.cache.get(&height) {
            return Ok(Some(hash));
        }

        let hash = chain.get_block_in_best_chain(height)?;

        if hash.is_some() && height > self.tip_height {
            self.tip_height = height;
        }

        // only cache block if at least as deep as `assume_final_depth`
        let assume_final_height = self.tip_height.saturating_sub(self.assume_final_depth);
        if height <= assume_final_height {
            if let Some(hash) = hash {
                self.cache.insert(height, hash);
            }
        }

        Ok(hash)
    }

    /// Determines whether the block of `block_id` is in the chain using the cache.
    ///
    /// This uses [`try_get_block`] internally.
    ///
    /// [`try_get_block`]: Self::try_get_block
    pub fn try_is_block_in_chain(&mut self, chain: C, block_id: BlockId) -> Result<bool, C::Error> {
        match self.try_get_block(chain, block_id.height)? {
            Some(hash) if hash == block_id.hash => Ok(true),
            _ => Ok(false),
        }
    }
}

impl<C: ChainOracle<Error = Infallible>> Cache<C> {
    /// Updates the `tip_height` with the [`ChainOracle`]'s tip.
    ///
    /// This is the no-error version of [`try_update_tip_height`].
    ///
    /// [`try_update_tip_height`]: Self::try_update_tip_height
    pub fn update_tip_height(&mut self, chain: C) {
        self.try_update_tip_height(chain)
            .expect("chain oracle error is infallible")
    }

    /// Get a block from the cache with the [`ChainOracle`] as fallback.
    ///
    /// This is the no-error version of [`try_get_block`].
    ///
    /// [`try_get_block`]: Self::try_get_block
    pub fn get_block(&mut self, chain: C, height: u32) -> Option<BlockHash> {
        self.try_get_block(chain, height)
            .expect("chain oracle error is infallible")
    }

    /// Determines whether the block at `block_id` is in the chain using the cache.
    ///
    /// This is the no-error version of [`try_is_block_in_chain`].
    ///
    /// [`try_is_block_in_chain`]: Self::try_is_block_in_chain
    pub fn is_block_in_best_chain(&mut self, chain: C, block_id: BlockId) -> bool {
        self.try_is_block_in_chain(chain, block_id)
            .expect("chain oracle error is infallible")
    }
}
