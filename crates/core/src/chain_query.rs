//! Sans-IO trait for tasks that need to query block data from a chain source.
//!
//! [`ChainQuery`] defines a cooperative protocol between a **task** (the implementor)
//! and a **driver** (the caller that has access to the chain source). The task
//! declares what block heights it needs, the driver fetches them, and the task
//! makes progress until it produces a final output.
//!
//! # The driver loop
//!
//! A correct driver calls [`poll`](ChainQuery::poll) in a loop, matching the returned
//! [`TaskProgress`]:
//!
//! ```ignore
//! loop {
//!     match task.poll() {
//!         TaskProgress::Advanced => continue,
//!         TaskProgress::Done => return task.finish(),
//!         TaskProgress::Query(heights) => {
//!             debug_assert!(!heights.is_empty());
//!             for h in heights {
//!                 let block = chain_source.get(h);
//!                 task.resolve_query(h, block);
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! # The block type `B`
//!
//! The generic parameter `B` is the block data the task receives from the driver.
//! It defaults to [`BlockHash`] but can be any type (e.g. [`Header`](bitcoin::block::Header))
//! depending on the chain source. A response of `None` means the chain source has
//! no block at that height.

use crate::BlockId;
use alloc::vec::Vec;
use bitcoin::BlockHash;

/// Progress indicator returned by [`ChainQuery::poll`].
///
/// This enum makes "stuck" states unrepresentable: the task must always either
/// make progress, request data, or declare itself done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProgress {
    /// Internal progress was made. The driver should call [`poll`](ChainQuery::poll) again.
    ///
    /// Implementations should return this at stage transitions and after processing
    /// individual items, rather than looping internally. This keeps `poll()` doing
    /// one unit of work per call, giving the driver observability into progress and
    /// an opportunity to cancel or log between steps.
    Advanced,
    /// The task needs block data at these heights. The driver should resolve each
    /// height via [`resolve_query`](ChainQuery::resolve_query) and then call
    /// [`poll`](ChainQuery::poll) again.
    ///
    /// The heights vector is guaranteed to be non-empty by correct implementations.
    Query(Vec<u32>),
    /// The task is complete. The driver should call [`finish`](ChainQuery::finish).
    Done,
}

/// A sans-IO task that queries block data by height from a chain source.
///
/// See the [module-level documentation](self) for the driver loop contract.
///
/// # Contract for implementors
///
/// - [`poll`](Self::poll) should do one logical unit of work and return:
///   - [`TaskProgress::Advanced`] when internal progress was made (e.g. a stage transition or
///     processing a single item). The driver will call `poll()` again.
///   - [`TaskProgress::Query`] when the task needs block data at one or more heights.
///   - [`TaskProgress::Done`] when all processing is complete.
/// - Prefer returning [`TaskProgress::Advanced`] over looping internally. This gives the driver
///   control between steps for progress reporting, cancellation, or logging.
/// - [`finish`](Self::finish) consumes the task and produces the final output. It should only be
///   called after `poll` returns [`TaskProgress::Done`].
pub trait ChainQuery<B = BlockHash> {
    /// The final output produced when the task completes.
    type Output;

    /// The chain tip that serves as the reference point for all queries.
    fn tip(&self) -> BlockId;

    /// Drive the task forward, returning its progress status.
    ///
    /// Each call should perform one logical unit of work. The driver calls this
    /// in a loop, matching the returned [`TaskProgress`].
    fn poll(&mut self) -> TaskProgress;

    /// Provides the block data (or `None`) for a previously requested `height`.
    ///
    /// A response of `None` signals that the chain source has no block at that
    /// height. The task records this so it can distinguish "not yet queried"
    /// from "queried but absent."
    fn resolve_query(&mut self, height: u32, response: Option<B>);

    /// Returns heights that have been requested but not yet resolved.
    ///
    /// This is useful for retrying failed queries or reporting progress.
    fn unresolved_queries<'a>(&'a self) -> impl Iterator<Item = u32> + 'a;

    /// Consumes the task and returns the final output.
    ///
    /// # Panics
    ///
    /// May panic if called before [`poll`](Self::poll) returns [`TaskProgress::Done`].
    fn finish(self) -> Self::Output;
}
