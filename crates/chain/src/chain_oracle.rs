use crate::BlockId;

/// Represents a service that tracks the blockchain.
///
/// The main method is [`is_block_in_chain`] which determines whether a given block of [`BlockId`]
/// is an ancestor of the `chain_tip`.
///
/// [`is_block_in_chain`]: Self::is_block_in_chain
#[deprecated(
    since = "0.23.3",
    note = "`ChainOracle` will be removed in a future release, replaced by `ChainQuery` in `bdk_core`. See: https://github.com/bitcoindevkit/bdk/pull/2038"
)]
pub trait ChainOracle {
    /// Error type.
    type Error: core::fmt::Debug;

    /// Determines whether `block` of [`BlockId`] exists as an ancestor of `chain_tip`.
    ///
    /// If `None` is returned, it means the implementation cannot determine whether `block` exists
    /// under `chain_tip`.
    fn is_block_in_chain(
        &self,
        block: BlockId,
        chain_tip: BlockId,
    ) -> Result<Option<bool>, Self::Error>;

    /// Get the best chain's chain tip.
    fn get_chain_tip(&self) -> Result<BlockId, Self::Error>;
}
