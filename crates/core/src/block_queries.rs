use crate::collections::BTreeMap;
use crate::{BlockId, ToBlockHash};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Tracks block-height queries: which heights have been requested,
/// which are still pending, and the resolved data.
///
/// This is a helper for [`ChainTask`](crate::ChainTask) implementors that need to
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

impl<B: ToBlockHash> BlockQueries<B> {
    /// Resolves a set of candidate blocks against the blocks fetched so far.
    ///
    /// Each candidate is a pair of an opaque payload and a [`BlockId`]. A candidate is *in-chain*
    /// when the block resolved at that `BlockId`'s height is present and its hash matches; the
    /// first such candidate's payload is returned as
    /// [`Confirmed`](BlockCandidateResolution::Confirmed).
    ///
    /// When no candidate is confirmed, the result tells the caller how to make progress:
    /// - [`Query`](BlockCandidateResolution::Query) — heights that still need fetching. These come
    ///   from [`request`](Self::request), so the list is *additive* (each height announced once)
    ///   and never empty.
    /// - [`Awaiting`](BlockCandidateResolution::Awaiting) — nothing new to request, but some
    ///   heights are still in-flight; wait for them to resolve.
    /// - [`NotConfirmed`](BlockCandidateResolution::NotConfirmed) — every candidate height is
    ///   resolved and none matched.
    ///
    /// `candidates` is cloned and traversed up to three times, so prefer a cheaply cloneable
    /// iterator (e.g. a `map` over a slice or set).
    pub fn resolve_candidates<C: Clone>(
        &mut self,
        candidates: impl Iterator<Item = (C, BlockId)> + Clone,
    ) -> BlockCandidateResolution<C> {
        // A candidate is confirmed only by a resolved, present block whose hash matches. Unresolved
        // (`None`), absent (`Some(None)`), or mismatched blocks do not confirm it.
        for (c, id) in candidates.clone() {
            if let Some(Some(b)) = self.get(id.height) {
                if b.to_blockhash() == id.hash {
                    return BlockCandidateResolution::Confirmed(c.clone());
                }
            }
        }

        let heights = self.request(candidates.clone().map(|(_, id)| id.height));
        if !heights.is_empty() {
            return BlockCandidateResolution::Query(heights);
        }

        // Nothing new to request: wait if any candidate height is still in-flight, otherwise all
        // are resolved and none matched.
        if candidates
            .clone()
            .any(|(_, id)| self.get(id.height).is_none())
        {
            return BlockCandidateResolution::Awaiting;
        }

        BlockCandidateResolution::NotConfirmed
    }
}

impl<B> Default for BlockQueries<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of [`BlockQueries::resolve_candidates`].
///
/// A "candidate" is some value `C` that claims to live at a particular [`BlockId`] (height +
/// hash). Resolution checks those claims against the blocks fetched so far and reports which of
/// four states applies. The generic `C` is carried through opaquely so the confirmed candidate can
/// be returned to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockCandidateResolution<C> {
    /// One candidate's block is resolved and its hash matches — this candidate is in-chain.
    Confirmed(C),
    /// These *newly requested* heights must be fetched before the candidates can be decided.
    /// Always non-empty.
    Query(Vec<u32>),
    /// Nothing new to request, but some candidate heights are still in-flight; the candidates
    /// can't be decided until the driver resolves them.
    Awaiting,
    /// Every candidate height is resolved and none match — no candidate is in-chain.
    NotConfirmed,
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::vec;
    use bitcoin::{hashes::Hash, BlockHash};

    fn bhash(n: u8) -> BlockHash {
        BlockHash::from_byte_array([n; 32])
    }

    fn id(height: u32, hash: u8) -> BlockId {
        BlockId {
            height,
            hash: bhash(hash),
        }
    }

    #[test]
    fn resolve_candidates_scenarios() {
        struct Case {
            name: &'static str,
            /// Pre-populate the `BlockQueries` to set up the scenario.
            setup: fn(&mut BlockQueries<BlockHash>),
            candidates: Vec<BlockId>,
            expected: BlockCandidateResolution<BlockId>,
            /// Heights still outstanding after the call.
            expected_unresolved: Vec<u32>,
        }

        let cases = [
            Case {
                name: "confirmed when the block is resolved and its hash matches",
                setup: |q| {
                    q.request([5].into_iter());
                    q.resolve(5, Some(bhash(5)));
                },
                candidates: vec![id(5, 5)],
                expected: BlockCandidateResolution::Confirmed(id(5, 5)),
                expected_unresolved: vec![],
            },
            Case {
                name: "confirmed picks the matching candidate among several",
                setup: |q| {
                    q.request([4, 5].into_iter());
                    q.resolve(4, Some(bhash(99))); // resolved but mismatched
                    q.resolve(5, Some(bhash(5))); // matches
                },
                candidates: vec![id(4, 4), id(5, 5)],
                expected: BlockCandidateResolution::Confirmed(id(5, 5)),
                expected_unresolved: vec![],
            },
            Case {
                name: "query for a height not yet requested",
                setup: |_q| {},
                candidates: vec![id(7, 7)],
                expected: BlockCandidateResolution::Query(vec![7]),
                expected_unresolved: vec![7], // the call records the request
            },
            Case {
                name: "awaiting when a requested height is still in-flight",
                setup: |q| {
                    q.request([7].into_iter());
                },
                candidates: vec![id(7, 7)],
                expected: BlockCandidateResolution::Awaiting,
                expected_unresolved: vec![7],
            },
            Case {
                name: "not confirmed when the block is resolved but mismatched",
                setup: |q| {
                    q.request([7].into_iter());
                    q.resolve(7, Some(bhash(99)));
                },
                candidates: vec![id(7, 7)],
                expected: BlockCandidateResolution::NotConfirmed,
                expected_unresolved: vec![],
            },
            Case {
                name: "not confirmed when the block is absent",
                setup: |q| {
                    q.request([7].into_iter());
                    q.resolve(7, None);
                },
                candidates: vec![id(7, 7)],
                expected: BlockCandidateResolution::NotConfirmed,
                expected_unresolved: vec![],
            },
        ];

        for case in cases {
            let mut q = BlockQueries::<BlockHash>::new();
            (case.setup)(&mut q);

            let res = q.resolve_candidates(case.candidates.iter().map(|id| (*id, *id)));
            assert_eq!(res, case.expected, "scenario: {}", case.name);
            assert!(
                q.unresolved().eq(case.expected_unresolved.iter().copied()),
                "scenario: {} (unresolved mismatch)",
                case.name
            );
        }
    }
}
