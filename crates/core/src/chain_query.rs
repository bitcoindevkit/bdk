//! Generic trait for query-based operations that require external blockchain data.
//!
//! The [`ChainQuery`] trait provides a standardized interface for implementing
//! algorithms that need to make queries to blockchain sources and process responses
//! in a sans-IO manner.

use crate::BlockId;
use alloc::vec::Vec;

/// A request containing block identifiers to check for confirmation in the chain.
///
/// The generic parameter `B` represents the block identifier type, which defaults to `BlockId`.
pub type ChainRequest<B = BlockId> = Vec<B>;

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
/// # Protocol
///
/// Callers must drive the task by calling [`next_query`](Self::next_query) and
/// [`resolve_query`](Self::resolve_query) in a loop. `resolve_query` must only be called
/// after `next_query` returns `Some`. Once `next_query` returns `None`, call
/// [`finish`](Self::finish) to get the output. Calling `resolve_query` or `finish` out of
/// sequence is a programming error.
///
/// # Type Parameters
///
/// * `B` - The type of block identifier used in queries (defaults to `BlockId`)
pub trait ChainQuery<B = BlockId> {
    /// The final output type produced when the query process is complete.
    type Output;

    /// Returns the chain tip used as the reference point for all queries.
    fn tip(&self) -> B;

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

    /// Completes the query process and returns the final output.
    ///
    /// This method should be called once [`next_query`](Self::next_query) returns `None`.
    /// It consumes `self` and produces the final output.
    fn finish(self) -> Self::Output;
}
