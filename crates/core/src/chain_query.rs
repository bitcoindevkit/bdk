//! Generic trait for query-based operations that require external blockchain data.
//!
//! The [`ChainQuery`] trait provides a standardized interface for implementing
//! algorithms that need to make queries to blockchain sources and process responses
//! in a sans-IO manner.

use crate::BlockId;
use alloc::vec::Vec;

/// A request to check which block identifiers are confirmed in the chain.
///
/// This is used to verify if specific blocks are part of the canonical chain.
/// The generic parameter `B` represents the block identifier type, which defaults to `BlockId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainRequest<B = BlockId> {
    /// The chain tip to use as reference for the query.
    pub chain_tip: B,
    /// The block identifiers to check for confirmation in the chain.
    pub block_ids: Vec<B>,
}

/// Response containing the best confirmed block identifier, if any.
///
/// Returns `Some(B)` if at least one of the requested blocks
/// is confirmed in the chain, or `None` if none are confirmed.
/// The generic parameter `B` represents the block identifier type, which defaults to `BlockId`.
pub type ChainResponse<B = BlockId> = Option<B>;

/// A trait for types that perform query-based operations against blockchain data.
///
/// This trait enables types to request blockchain information via queries and process
/// responses in a decoupled, sans-IO manner. It's particularly useful for algorithms
/// that need to interact with blockchain oracles, chain sources, or other blockchain
/// data providers without directly performing I/O.
///
/// # Type Parameters
///
/// * `B` - The type of block identifier used in queries (defaults to `BlockId`)
pub trait ChainQuery<B = BlockId> {
    /// The final output type produced when the query process is complete.
    type Output;

    /// Returns the next query needed, if any.
    ///
    /// This method should return `Some(request)` if more information is needed,
    /// or `None` if no more queries are required.
    fn next_query(&mut self) -> Option<ChainRequest<B>>;

    /// Resolves a query with the given response.
    ///
    /// This method processes the response to a previous query request and updates
    /// the internal state accordingly.
    fn resolve_query(&mut self, response: ChainResponse<B>);

    /// Returns true if the query process is complete and ready to finish.
    ///
    /// The default implementation returns `true` when there are no more queries needed.
    /// Implementors can override this for more specific behavior if needed.
    fn is_finished(&mut self) -> bool {
        self.next_query().is_none()
    }

    /// Completes the query process and returns the final output.
    ///
    /// This method should be called when `is_finished` returns `true`.
    /// It consumes `self` and produces the final output.
    fn finish(self) -> Self::Output;
}
