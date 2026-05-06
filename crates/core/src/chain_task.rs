//! Sans-IO trait for tasks that need to query block data from a chain source.
//!
//! See [`ChainTask`] for the cooperative protocol between a **task** (the implementor)
//! and a **driver** (the caller that has access to the chain source), including the
//! driver loop contract.

use crate::BlockId;
use alloc::vec::Vec;
use bitcoin::BlockHash;

/// Progress indicator returned by [`ChainTask::poll`].
///
/// This enum makes "stuck" states unrepresentable: the task must always either
/// make progress, request data, declare itself blocked on in-flight data, or
/// declare itself done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProgress {
    /// Internal progress was made. The driver should call [`poll`](ChainTask::poll) again.
    ///
    /// Implementations should return this at stage transitions and after processing
    /// individual items, rather than looping internally. This keeps `poll()` doing
    /// one unit of work per call, giving the driver observability into progress and
    /// an opportunity to cancel or log between steps.
    Advanced,
    /// The task needs block data at these **newly requested** heights. The driver should
    /// resolve each height via [`resolve_query`](ChainTask::resolve_query) and call
    /// [`poll`](ChainTask::poll) again.
    ///
    /// This variant is *additive*: a given height appears in exactly one `Query`, when it is
    /// first needed. Implementations dedup against already-requested heights, so the vector is
    /// guaranteed to be non-empty and to contain only heights the driver has not seen before.
    /// A driver can therefore launch a fetch for each height without tracking duplicates.
    Query(Vec<u32>),
    /// The task cannot make progress until previously requested heights are resolved, and it
    /// has no *new* heights to request. The driver should resolve at least one outstanding
    /// height (see [`unresolved_queries`](ChainTask::unresolved_queries)) and call
    /// [`poll`](ChainTask::poll) again.
    ///
    /// A synchronous driver that resolves every [`Query`](Self::Query) height before re-polling
    /// never observes this variant (nothing is ever in-flight). It exists for streaming drivers
    /// that issue fetches without blocking and poll opportunistically: `Query` says "launch I/O",
    /// `AwaitingQueries` says "block until some lands".
    AwaitingQueries,
    /// The task is complete. The driver should call [`finish`](ChainTask::finish).
    Done,
}

/// A sans-IO task that queries block data by height from a chain source.
///
/// `ChainTask` defines a cooperative protocol between a **task** (the implementor)
/// and a **driver** (the caller that has access to the chain source). The task
/// declares what block heights it needs, the driver fetches them, and the task
/// makes progress until it produces a final output.
///
/// # The driver loop
///
/// A correct driver calls [`poll`](Self::poll) in a loop, matching the returned
/// [`TaskProgress`]. A simple **synchronous** driver resolves each query inline:
///
/// ```ignore
/// loop {
///     match task.poll() {
///         TaskProgress::Advanced => continue,
///         TaskProgress::Done => return task.finish(),
///         TaskProgress::Query(heights) => {
///             debug_assert!(!heights.is_empty());
///             for h in heights {
///                 let block = chain_source.get(h);
///                 task.resolve_query(h, block);
///             }
///         }
///         // A synchronous driver resolves every query before re-polling, so nothing is
///         // ever in-flight and this variant cannot occur.
///         TaskProgress::AwaitingQueries => unreachable!(),
///     }
/// }
/// ```
///
/// A **streaming** driver instead launches fetches on [`Query`](TaskProgress::Query) without
/// blocking and re-polls; when the task returns [`AwaitingQueries`](TaskProgress::AwaitingQueries)
/// it blocks on the next fetch to complete, resolves it, and re-polls.
///
/// # The block type `B`
///
/// The generic parameter `B` is the block data the task receives from the driver.
/// It defaults to [`BlockHash`] but can be any type (e.g. [`Header`](bitcoin::block::Header))
/// depending on the chain source. A response of `None` means the chain source has
/// no block at that height.
///
/// # Contract for implementors
///
/// - [`poll`](Self::poll) should do one logical unit of work and return:
///   - [`TaskProgress::Advanced`] when internal progress was made (e.g. a stage transition or
///     processing a single item). The driver will call `poll()` again.
///   - [`TaskProgress::Query`] when the task needs block data at one or more *newly requested*
///     heights. Dedup against already-requested heights so each height is announced once and the
///     vector is never empty.
///   - [`TaskProgress::AwaitingQueries`] when the task has no new heights to request but cannot
///     progress until previously requested heights are resolved.
///   - [`TaskProgress::Done`] when all processing is complete.
/// - Prefer returning [`TaskProgress::Advanced`] over looping internally. This gives the driver
///   control between steps for progress reporting, cancellation, or logging.
/// - [`finish`](Self::finish) consumes the task and produces the final output. It should only be
///   called after `poll` returns [`TaskProgress::Done`].
pub trait ChainTask<B = BlockHash> {
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
