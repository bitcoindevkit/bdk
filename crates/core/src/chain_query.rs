//! Trait for query-based canonicalization against blockchain data.
//!
//! The [`ChainQuery`] trait provides a sans-IO interface for algorithms that
//! need to verify block confirmations against a chain source.

use crate::BlockId;
use alloc::vec::Vec;

/// A request containing [`BlockId`]s to check for confirmation in the chain.
pub type ChainRequest = Vec<BlockId>;

/// Response containing the best confirmed [`BlockId`], if any.
pub type ChainResponse = Option<BlockId>;

/// A trait for types that verify block confirmations against blockchain data.
///
/// This trait enables a sans-IO loop: the caller drives the task by repeatedly
/// calling [`next_query`](Self::next_query) and [`resolve_query`](Self::resolve_query).
/// Once `next_query` returns `None`, call [`finish`](Self::finish) to get the output.
///
/// `resolve_query` must only be called after `next_query` returns `Some`.
/// Calling `resolve_query` or `finish` out of sequence is a programming error.
pub trait ChainQuery {
    /// The final output type produced when the query process is complete.
    type Output;

    /// Returns the chain tip used as the reference point for all queries.
    fn tip(&self) -> BlockId;

    /// Returns the next query needed, or `None` if no more queries are required.
    fn next_query(&mut self) -> Option<ChainRequest>;

    /// Resolves a query with the given response.
    fn resolve_query(&mut self, response: ChainResponse);

    /// Completes the query process and returns the final output.
    ///
    /// This should be called once [`next_query`](Self::next_query) returns `None`.
    fn finish(self) -> Self::Output;
}
