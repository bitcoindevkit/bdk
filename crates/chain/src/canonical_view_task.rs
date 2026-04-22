//! Phase 2 task: resolves canonical reasons into chain positions.

use crate::canonical_task::{CanonicalReason, ObservedIn};
use crate::collections::{HashMap, VecDeque};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use bdk_core::{BlockId, BlockQueries, ChainQuery, TaskProgress, ToBlockHash, ToBlockTime};
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
            if let Some(entry) = txs.get(txid) {
                match &entry.pos {
                    CanonicalReason::Anchor {
                        anchor,
                        descendant: None,
                    } => {
                        // Non-transitive anchor — already resolved.
                        direct_anchors.insert(*txid, anchor.clone());
                    }
                    CanonicalReason::Anchor { .. } | CanonicalReason::Assumed { .. } => {
                        // Transitive or assumed — needs anchor verification.
                        if let Some(anchors) = all_anchors.get(txid) {
                            unprocessed_anchor_checks.push_back((*txid, anchors));
                        }
                    }
                    CanonicalReason::ObservedIn { .. } => {}
                }
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
    /// When enabled, the task will fetch additional blocks needed to compute MTP values
    /// for each confirmed height and the tip height. The resulting [`CanonicalView`] will
    /// have per-tx MTP on [`CanonicalTx::mtp`](crate::CanonicalTx::mtp) and the tip MTP
    /// accessible via [`tip_mtp()`](crate::Canonical::tip_mtp).
    pub fn with_mtp(mut self) -> Self {
        self.extract_time = Some(B::to_blocktime);
        self
    }
}

impl<'g, A: Anchor, B: ToBlockHash> ChainQuery<B> for CanonicalViewTask<'g, A, B> {
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
                    let mut best_anchor = Option::<A>::None;
                    let mut has_unresolved_heights = false;

                    for a in anchors.iter() {
                        let h = a.anchor_block().height;
                        match self.queries.get(h) {
                            Some(Some(b)) => {
                                if b.to_blockhash() == a.anchor_block().hash {
                                    best_anchor = Some(a.clone());
                                    break;
                                }
                            }
                            Some(None) => {}
                            None => {
                                has_unresolved_heights = true;
                            }
                        }
                    }

                    if let Some(anchor) = best_anchor {
                        self.direct_anchors.insert(txid, anchor);
                        return TaskProgress::Advanced;
                    }

                    if has_unresolved_heights {
                        let heights = self
                            .queries
                            .request(anchors.iter().map(|a| a.anchor_block().height));
                        self.unprocessed_anchor_checks.push_front((txid, anchors));
                        return TaskProgress::Query(heights);
                    }

                    // No confirmed anchor found for this tx
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

                // Collect all MTP heights needed.
                let mut mtp_heights = BTreeSet::new();
                mtp_heights.insert(self.tip.height);
                for anchor in self.direct_anchors.values() {
                    mtp_heights.insert(anchor.confirmation_height_upper_bound());
                }

                let needed = self
                    .queries
                    .request(mtp_heights.iter().flat_map(|&h| h.saturating_sub(10)..=h));

                if needed.is_empty() {
                    self.current_stage = ViewStage::Finished;
                    return TaskProgress::Advanced;
                }

                TaskProgress::Query(needed)
            }
            ViewStage::Finished => TaskProgress::Done,
        }
    }

    fn resolve_query(&mut self, height: u32, block: Option<B>) {
        self.queries.resolve(height, block);
    }

    fn finish(self) -> Self::Output {
        // Helper: compute MTP for a given height from the blocks map.
        let compute_mtp_at = |h: u32, f: fn(&B) -> u32| -> Option<u32> {
            let start = h.saturating_sub(10);
            let mut ts: Vec<u32> = (start..=h)
                .map(|mtp_h| self.queries.get(mtp_h).and_then(|b| b.as_ref()).map(f))
                .collect::<Option<Vec<_>>>()?;
            ts.sort_unstable();
            Some(ts[ts.len() / 2])
        };

        let extract_time = self.extract_time;

        // Compute tip MTP.
        let tip_mtp = extract_time.and_then(|f| compute_mtp_at(self.tip.height, f));

        let mut view_order = Vec::new();
        let mut view_txs = HashMap::new();

        for txid in &self.canonical_order {
            if let Some(CanonicalEntry {
                tx, pos: reason, ..
            }) = self.canonical_txs.get(txid)
            {
                view_order.push(*txid);

                // Get transaction node for first_seen/last_seen info
                let tx_node = match self.tx_graph.get_tx_node(*txid) {
                    Some(tx_node) => tx_node,
                    None => {
                        debug_assert!(false, "tx node must exist!");
                        continue;
                    }
                };

                // Determine chain position and per-tx MTP. For confirmed txs, prefer
                // direct_anchors (which includes both non-transitive anchors and resolved
                // anchors for transitive/assumed txs).
                let (chain_position, mtp) = if let Some(anchor) = self.direct_anchors.get(txid) {
                    let mtp = extract_time
                        .and_then(|f| compute_mtp_at(anchor.confirmation_height_upper_bound(), f));
                    (
                        ChainPosition::Confirmed {
                            anchor,
                            transitively: None,
                        },
                        mtp,
                    )
                } else {
                    match reason {
                        CanonicalReason::Anchor { anchor, descendant } => {
                            let mtp = extract_time.and_then(|f| {
                                compute_mtp_at(anchor.confirmation_height_upper_bound(), f)
                            });
                            (
                                ChainPosition::Confirmed {
                                    anchor,
                                    transitively: *descendant,
                                },
                                mtp,
                            )
                        }
                        CanonicalReason::ObservedIn {
                            observed_in: ObservedIn::Mempool(last_seen),
                            ..
                        } => (
                            ChainPosition::Unconfirmed {
                                first_seen: tx_node.first_seen,
                                last_seen: Some(*last_seen),
                            },
                            None,
                        ),
                        _ => (
                            ChainPosition::Unconfirmed {
                                first_seen: tx_node.first_seen,
                                last_seen: tx_node.last_seen,
                            },
                            None,
                        ),
                    }
                };

                view_txs.insert(
                    *txid,
                    CanonicalEntry {
                        tx: tx.clone(),
                        pos: chain_position.cloned(),
                        mtp,
                    },
                );
            }
        }

        CanonicalView::new(self.tip, view_order, view_txs, self.spends.clone(), tip_mtp)
    }
}
