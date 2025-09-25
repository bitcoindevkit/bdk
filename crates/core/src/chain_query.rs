//! Generic trait for query-based operations that require external blockchain data.
//!
//! The [`ChainQuery`] trait provides a standardized interface for implementing
//! algorithms that need to make queries to blockchain sources and process responses
//! in a sans-IO manner.

/// A trait for types that perform query-based operations against blockchain data.
///
/// This trait enables types to request blockchain information via queries and process
/// responses in a decoupled, sans-IO manner. It's particularly useful for algorithms
/// that need to interact with blockchain oracles, chain sources, or other blockchain
/// data providers without directly performing I/O.
///
/// # Type Parameters
///
/// * `Request` - The type of query request that can be made
/// * `Response` - The type of response expected for queries
/// * `Context` - The type of context needed for finalization (e.g., `BlockId` for chain tip)
/// * `Result` - The final result type produced when the query process is complete
pub trait ChainQuery {
    /// The type of query request that can be made.
    type Request;

    /// The type of response expected for queries.
    type Response;

    /// The type of context needed for finalization.
    ///
    /// This could be `BlockId` for algorithms needing chain tip information,
    /// `()` for algorithms that don't need additional context, or any other
    /// type specific to the implementation's needs.
    type Context;

    /// The final result type produced when the query process is complete.
    type Result;

    /// Returns the next query needed, if any.
    ///
    /// This method should return `Some(request)` if more information is needed,
    /// or `None` if no more queries are required.
    fn next_query(&mut self) -> Option<Self::Request>;

    /// Resolves a query with the given response.
    ///
    /// This method processes the response to a previous query request and updates
    /// the internal state accordingly.
    fn resolve_query(&mut self, response: Self::Response);

    /// Returns true if the query process is complete and ready to finish.
    ///
    /// The default implementation returns `true` when there are no more queries needed.
    /// Implementors can override this for more specific behavior if needed.
    fn is_finished(&mut self) -> bool {
        self.next_query().is_none()
    }

    /// Completes the query process and returns the final result.
    ///
    /// This method should be called when `is_finished` returns `true`.
    /// It consumes `self` and produces the final result.
    ///
    /// The `context` parameter provides implementation-specific context
    /// needed for finalization.
    fn finish(self, context: Self::Context) -> Self::Result;
}
