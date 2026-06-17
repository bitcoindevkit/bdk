//! Phase 2 task: resolves canonical reasons into chain positions.

use crate::canonical_task::{CanonicalReason, ObservedIn};
use crate::collections::{HashMap, VecDeque};
use crate::tx_graph::TxDescendants;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use bdk_core::{
    BlockCandidateResolution, BlockId, BlockQueries, ChainTask, TaskProgress, ToBlockHash,
    ToBlockTime,
};
use bitcoin::{OutPoint, Txid};

use crate::{canonical::CanonicalEntry, Anchor, CanonicalView, ChainPosition, TxGraph};

/// Represents the current stage of view task processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ViewStage {
    /// Verifying anchors for transitively anchored transactions.
    #[default]
    ResolvingPositions,
    /// Fetching blocks needed for MTP computation.
    FetchingMtpBlocks,
    /// All processing is complete.
    Finished,
}

/// Resolves [`CanonicalReason`]s into [`ChainPosition`]s.
///
/// This task implements the second phase of canonicalization: given a set of canonical
/// transactions with their reasons (from [`CanonicalTask`](crate::CanonicalTask)), it resolves each
/// reason into a concrete [`ChainPosition`] (confirmed or unconfirmed). For transitively
/// anchored transactions, it queries the chain to check if they have their own direct
/// anchors.
///
/// When `with_mtp()` is called, this task also computes median-time-past (MTP) values
/// for confirmed heights and stores them in the resulting [`CanonicalView`].
pub struct CanonicalViewTask<'g, A, B> {
    tx_graph: &'g TxGraph<A>,
    tip: BlockId,

    queries: BlockQueries<B>,

    canonical_order: Vec<Txid>,
    canonical_txs: HashMap<Txid, CanonicalEntry<CanonicalReason<A>>>,
    spends: HashMap<OutPoint, Txid>,
    unprocessed_anchor_checks: VecDeque<(Txid, &'g BTreeSet<A>)>,
    direct_anchors: HashMap<Txid, A>,

    // MTP support — `extract_time` being `Some` means MTP is enabled.
    extract_time: Option<fn(&B) -> u32>,

    current_stage: ViewStage,
}

impl<'g, A: Anchor, B> CanonicalViewTask<'g, A, B> {
    /// Creates a new [`CanonicalViewTask`].
    ///
    /// Accepts canonical transaction data, a reference to the [`TxGraph`], and blocks
    /// already fetched during phase 1 (to avoid redundant queries).
    pub(crate) fn new(
        tx_graph: &'g TxGraph<A>,
        tip: BlockId,
        order: Vec<Txid>,
        txs: HashMap<Txid, CanonicalEntry<CanonicalReason<A>>>,
        spends: HashMap<OutPoint, Txid>,
        queries: BlockQueries<B>,
    ) -> Self {
        let all_anchors = tx_graph.all_anchors();

        let mut unprocessed_anchor_checks = VecDeque::new();
        let mut direct_anchors = HashMap::new();
        for txid in &order {
            // `mark_canonical` inserts into the canonical map and pushes to the canonical order
            // in the same call, and only ever removes from the map before that push, so every
            // txid in `order` has an entry in `txs`.
            let entry = match txs.get(txid) {
                Some(entry) => entry,
                None => {
                    debug_assert!(false, "every txid in `order` must have an entry in `txs`");
                    continue;
                }
            };
            match &entry.pos {
                CanonicalReason::Anchor {
                    anchor,
                    descendant: None,
                } => {
                    // Non-transitive anchor — already resolved.
                    direct_anchors.insert(*txid, anchor.clone());
                }
                CanonicalReason::Anchor { .. } | CanonicalReason::Assumed { .. } => {
                    // Transitive or assumed — needs anchor verification. The tx may have no
                    // anchors of its own: a transitive reason carries the *descendant's* anchor,
                    // and an assumed one needs no chain evidence at all.
                    if let Some(anchors) = all_anchors.get(txid) {
                        unprocessed_anchor_checks.push_back((*txid, anchors));
                    }
                }
                CanonicalReason::ObservedIn { .. } => {}
            }
        }

        Self {
            tx_graph,
            tip,
            queries,
            canonical_order: order,
            canonical_txs: txs,
            spends,
            unprocessed_anchor_checks,
            direct_anchors,
            extract_time: None,
            current_stage: ViewStage::default(),
        }
    }
}

impl<'g, A: Anchor, B: ToBlockHash + ToBlockTime> CanonicalViewTask<'g, A, B> {
    /// Enable MTP (median-time-past) computation.
    ///
    /// When enabled, the task will fetch additional blocks needed to compute MTP values.
    /// For each confirmed tx it computes the MTP of the block *preceding* the confirmation
    /// height (per BIP-68, matching Bitcoin Core's `CalculateSequenceLocks`), exposed on
    /// [`CanonicalTxOut::prev_mtp`](crate::CanonicalTxOut::prev_mtp). For the tip it computes
    /// MTP(tip.height) (per BIP-113), accessible via
    /// [`tip_mtp()`](crate::Canonical::tip_mtp).
    pub fn with_mtp(mut self) -> Self {
        self.extract_time = Some(B::to_blocktime);
        self
    }
}

impl<'g, A: Anchor, B: ToBlockHash> ChainTask<B> for CanonicalViewTask<'g, A, B> {
    type Output = CanonicalView<A>;

    fn tip(&self) -> BlockId {
        self.tip
    }

    fn unresolved_queries<'a>(&'a self) -> impl Iterator<Item = u32> + 'a {
        self.queries.unresolved()
    }

    fn poll(&mut self) -> TaskProgress {
        match self.current_stage {
            ViewStage::ResolvingPositions => {
                if let Some((txid, anchors)) = self.unprocessed_anchor_checks.pop_front() {
                    match self
                        .queries
                        .resolve_candidates(anchors.iter().map(|a| (a, a.anchor_block())))
                    {
                        BlockCandidateResolution::Confirmed(anchor) => {
                            self.direct_anchors.insert(txid, anchor.clone());
                        }
                        BlockCandidateResolution::Query(heights) => {
                            self.unprocessed_anchor_checks.push_front((txid, anchors));
                            return TaskProgress::Query(heights);
                        }
                        BlockCandidateResolution::Awaiting => {
                            self.unprocessed_anchor_checks.push_front((txid, anchors));
                            return TaskProgress::Blocked;
                        }
                        // No anchor confirms this tx; leave it without a direct anchor.
                        BlockCandidateResolution::NotConfirmed => {}
                    }
                    TaskProgress::Advanced
                } else {
                    self.current_stage = ViewStage::FetchingMtpBlocks;
                    TaskProgress::Advanced
                }
            }
            ViewStage::FetchingMtpBlocks => {
                if self.extract_time.is_none() {
                    self.current_stage = ViewStage::Finished;
                    return TaskProgress::Advanced;
                }

                // Collect all MTP heights needed (an 11-block window per target height).
                //
                // For the tip we need MTP(tip.height) (BIP-113). For each confirmed tx we need
                // the MTP of the block *preceding* the confirmation height (BIP-68), so we take
                // `confirmation_height - 1` (saturating at genesis) — the same value read in
                // `finish`.
                //
                // A `BTreeSet` rather than a `Vec`: the windows overlap heavily (two txs in one
                // block give identical windows, adjacent blocks share 10 of 11 heights), and this
                // is rebuilt on every poll of this stage. It also makes the emitted `Query`
                // ascending and deterministic — `direct_anchors` is a `HashMap`, so iterating its
                // values would otherwise vary the height order between runs.
                let required: BTreeSet<u32> = core::iter::once(self.tip.height)
                    .chain(
                        self.direct_anchors
                            .values()
                            .map(|a| a.confirmation_height_upper_bound().saturating_sub(1)),
                    )
                    .flat_map(|h| h.saturating_sub(10)..=h)
                    .collect();

                // `request` is additive: a non-empty result is new work to fetch.
                let needed = self.queries.request(required.iter().copied());
                if !needed.is_empty() {
                    return TaskProgress::Query(needed);
                }

                // No new heights. If any required height is still in-flight, wait for the driver;
                // otherwise every MTP block is resolved and we can finish.
                if required.iter().any(|h| self.queries.get(*h).is_none()) {
                    return TaskProgress::Blocked;
                }

                self.current_stage = ViewStage::Finished;
                TaskProgress::Advanced
            }
            ViewStage::Finished => TaskProgress::Done,
        }
    }

    fn resolve_query(&mut self, height: u32, block: Option<B>) {
        self.queries.resolve(height, block);
    }

    fn finish(self) -> Self::Output {
        // Helper: compute MTP for a given height from the blocks map. Returns `None` when MTP
        // was not enabled, so callers do not each repeat that check.
        let try_compute_mtp_at = |h: u32| -> Option<u32> {
            let extract_time = self.extract_time?;
            let start = h.saturating_sub(10);
            let mut ts: Vec<u32> = (start..=h)
                .map(|mtp_h| {
                    self.queries
                        .get(mtp_h)
                        .and_then(|b| b.as_ref())
                        .map(extract_time)
                })
                .collect::<Option<Vec<_>>>()?;
            ts.sort_unstable();
            Some(ts[ts.len() / 2])
        };

        // Compute tip MTP.
        let tip_mtp = try_compute_mtp_at(self.tip.height);

        let mut view_order = Vec::new();
        let mut view_txs = HashMap::<Txid, CanonicalEntry<_>>::new();

        for txid in &self.canonical_order {
            let (tx, reason) = match self.canonical_txs.get(txid) {
                Some(CanonicalEntry { tx, pos, .. }) => (tx, pos),
                None => {
                    debug_assert!(false, "missing entry in `canonical_txs`");
                    continue;
                }
            };

            view_order.push(*txid);

            // Get transaction node for first_seen/last_seen info.
            let tx_node = match self.tx_graph.get_tx_node(*txid) {
                Some(tx_node) => tx_node,
                None => {
                    debug_assert!(false, "tx node must exist!");
                    continue;
                }
            };

            // Handle directly anchored txs first.
            if let Some(anchor) = self.direct_anchors.get(txid) {
                let tx = tx.clone();
                let pos = ChainPosition::Confirmed {
                    anchor: anchor.clone(),
                    transitively: None,
                };
                // BIP-68 evaluates relative timelocks against the MTP of the block *before* the
                // coin's height (Core's `std::max(nCoinHeight - 1, 0)`), so compute MTP at
                // `confirmation_height - 1`, saturating to genesis for coins at height 0.
                let prev_mtp =
                    try_compute_mtp_at(anchor.confirmation_height_upper_bound().saturating_sub(1));
                view_txs.insert(*txid, CanonicalEntry { tx, pos, prev_mtp });
                continue;
            }

            let pos = match reason {
                // Since the canonicalization algorithm processes assumed txs first (and therefore
                // all their ancestors), some assumed txs may actually be confirmed in the best
                // chain. Therefore we need to check all assumed txs if themselves, or any
                // descendant of theirs, is anchored in the best chain.
                CanonicalReason::Assumed { .. } => {
                    TxDescendants::new_include_root(
                        self.tx_graph,
                        *txid,
                        // Only explore canonical descendants.
                        |_, desc_txid| -> Option<Txid> {
                            self.canonical_txs
                                .contains_key(&desc_txid)
                                .then_some(desc_txid)
                        },
                    )
                    // Find the earliest instance of an anchored tx.
                    .find_map(|desc_txid| {
                        self.direct_anchors
                            .get(&desc_txid)
                            .map(|anchor| ChainPosition::Confirmed {
                                anchor: anchor.clone(),
                                transitively: Some(desc_txid),
                            })
                    })
                    .unwrap_or(ChainPosition::Unconfirmed {
                        first_seen: tx_node.first_seen,
                        last_seen: tx_node.last_seen,
                    })
                }
                CanonicalReason::Anchor { anchor, descendant } => ChainPosition::Confirmed {
                    anchor: anchor.clone(),
                    transitively: *descendant,
                },
                CanonicalReason::ObservedIn { observed_in, .. } => ChainPosition::Unconfirmed {
                    first_seen: tx_node.first_seen,
                    // `last_seen` is a mempool sighting. `ObservedIn::Block` means the tx was
                    // only ever seen in a block that is not in the best chain, so there is no
                    // mempool sighting to report — a tx that re-entered the mempool would be
                    // `ObservedIn::Mempool` instead.
                    last_seen: match observed_in {
                        ObservedIn::Block(_) => None,
                        ObservedIn::Mempool(seen_at) => Some(*seen_at),
                    },
                },
            };
            view_txs.insert(
                *txid,
                CanonicalEntry {
                    tx: tx.clone(),
                    pos,
                    // No MTP: this stage handles txs without a direct anchor, so even when the
                    // position is `Confirmed` (transitively, via a descendant's anchor) this
                    // tx's own confirmation height is unknown and there is no block to median
                    // around.
                    prev_mtp: None,
                },
            );
        }

        CanonicalView::new(self.tip, view_order, view_txs, self.spends, tip_mtp)
    }
}
