//! Phase 2 task: resolves canonical reasons into chain positions.

use crate::canonical_task::{CanonicalReason, ObservedIn};
use crate::collections::{HashMap, VecDeque};
use crate::tx_graph::TxDescendants;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;

use bdk_core::{BlockId, ChainQuery, ChainRequest, ChainResponse};
use bitcoin::{OutPoint, Transaction, Txid};

use crate::{Anchor, CanonicalView, ChainPosition, TxGraph};

/// Represents the current stage of view task processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ViewStage {
    /// Processing transactions to resolve their chain positions.
    #[default]
    ResolvingPositions,
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
pub struct CanonicalViewTask<'g, A> {
    tx_graph: &'g TxGraph<A>,
    tip: BlockId,

    /// Transactions in canonical order with their reasons.
    canonical_order: Vec<Txid>,
    canonical_txs: HashMap<Txid, (Arc<Transaction>, CanonicalReason<A>)>,
    spends: HashMap<OutPoint, Txid>,

    /// Transactions that need anchor verification (transitively anchored).
    unprocessed_anchor_checks: VecDeque<(Txid, &'g BTreeSet<A>)>,

    /// Resolved direct anchors for transitively anchored transactions.
    direct_anchors: HashMap<Txid, A>,

    current_stage: ViewStage,
}

impl<'g, A: Anchor> CanonicalViewTask<'g, A> {
    /// Creates a new [`CanonicalViewTask`].
    ///
    /// Accepts canonical transaction data and a reference to the [`TxGraph`].
    /// Scans transactions to find those needing anchor verification.
    pub fn new(
        tx_graph: &'g TxGraph<A>,
        tip: BlockId,
        order: Vec<Txid>,
        txs: HashMap<Txid, (Arc<Transaction>, CanonicalReason<A>)>,
        spends: HashMap<OutPoint, Txid>,
    ) -> Self {
        let all_anchors = tx_graph.all_anchors();

        let mut unprocessed_anchor_checks = VecDeque::new();
        for txid in &order {
            if let Some((_, reason)) = txs.get(txid) {
                if matches!(reason, CanonicalReason::ObservedIn { .. }) {
                    continue;
                }
                if reason.is_transitive() || reason.is_assumed() {
                    if let Some(anchors) = all_anchors.get(txid) {
                        unprocessed_anchor_checks.push_back((*txid, anchors));
                    }
                }
            }
        }

        Self {
            tx_graph,
            tip,
            canonical_order: order,
            canonical_txs: txs,
            spends,
            unprocessed_anchor_checks,
            direct_anchors: HashMap::new(),
            current_stage: ViewStage::default(),
        }
    }
}

impl<'g, A: Anchor> ChainQuery for CanonicalViewTask<'g, A> {
    type Output = CanonicalView<A>;

    fn tip(&self) -> BlockId {
        self.tip
    }

    fn next_query(&mut self) -> Option<ChainRequest> {
        loop {
            match self.current_stage {
                ViewStage::ResolvingPositions => {
                    if let Some((_txid, anchors)) = self.unprocessed_anchor_checks.front() {
                        let block_ids =
                            anchors.iter().map(|anchor| anchor.anchor_block()).collect();
                        return Some(block_ids);
                    }
                }
                ViewStage::Finished => return None,
            }

            self.current_stage = ViewStage::Finished;
        }
    }

    fn resolve_query(&mut self, response: ChainResponse) {
        match self.current_stage {
            ViewStage::ResolvingPositions => {
                if let Some((txid, anchors)) = self.unprocessed_anchor_checks.pop_front() {
                    let best_anchor = response.and_then(|block_id| {
                        anchors
                            .iter()
                            .find(|anchor| anchor.anchor_block() == block_id)
                            .cloned()
                    });

                    if let Some(best_anchor) = best_anchor {
                        self.direct_anchors.insert(txid, best_anchor);
                    }
                }
            }
            ViewStage::Finished => {
                debug_assert!(false, "resolve_query called in Finished stage");
            }
        }
    }

    fn finish(self) -> Self::Output {
        let mut view_order = Vec::new();
        let mut view_txs = HashMap::new();

        for txid in &self.canonical_order {
            if let Some((tx, reason)) = self.canonical_txs.get(txid) {
                view_order.push(*txid);

                // Get transaction node for first_seen/last_seen info
                let tx_node = match self.tx_graph.get_tx_node(*txid) {
                    Some(tx_node) => tx_node,
                    None => {
                        debug_assert!(false, "tx node must exist!");
                        continue;
                    }
                };

                // Determine chain position based on reason
                let chain_position = match reason {
                    CanonicalReason::Assumed { descendant } => {
                        match self.direct_anchors.get(txid) {
                            // it has a direct anchor found
                            // regardless if it's directly or transitively assumed canonical
                            Some(anchor) => ChainPosition::Confirmed {
                                anchor,
                                transitively: None,
                            },
                            None => match descendant {
                                // transitively assumed canonical, walk through descendants to find
                                // the first confirmed one.
                                Some(_descendant) => {
                                    match TxDescendants::new_exclude_root(
                                        self.tx_graph,
                                        *txid,
                                        // ensure descendant is canonical
                                        |_, desc_txid| -> Option<Txid> {
                                            self.canonical_txs
                                                .contains_key(&desc_txid)
                                                .then_some(desc_txid)
                                        },
                                    )
                                    // ensure descendant has direct anchor
                                    .filter_map(|desc_txid| {
                                        self.direct_anchors.get(&desc_txid).map(|a| (desc_txid, a))
                                    })
                                    .next()
                                    {
                                        Some((desc_txid, anchor)) => ChainPosition::Confirmed {
                                            anchor,
                                            transitively: Some(desc_txid),
                                        },
                                        None => ChainPosition::Unconfirmed {
                                            first_seen: tx_node.first_seen,
                                            last_seen: tx_node.last_seen,
                                        },
                                    }
                                }
                                // directly assumed canonical, no direct anchor found.
                                None => ChainPosition::Unconfirmed {
                                    first_seen: tx_node.first_seen,
                                    last_seen: tx_node.last_seen,
                                },
                            },
                        }
                    }
                    CanonicalReason::Anchor { anchor, descendant } => match descendant {
                        Some(_) => match self.direct_anchors.get(txid) {
                            Some(anchor) => ChainPosition::Confirmed {
                                anchor,
                                transitively: None,
                            },
                            None => ChainPosition::Confirmed {
                                anchor,
                                transitively: *descendant,
                            },
                        },
                        None => ChainPosition::Confirmed {
                            anchor,
                            transitively: None,
                        },
                    },
                    CanonicalReason::ObservedIn { observed_in, .. } => match observed_in {
                        ObservedIn::Mempool(last_seen) => ChainPosition::Unconfirmed {
                            first_seen: tx_node.first_seen,
                            last_seen: Some(*last_seen),
                        },
                        ObservedIn::Block(_) => ChainPosition::Unconfirmed {
                            first_seen: tx_node.first_seen,
                            last_seen: None,
                        },
                    },
                };

                view_txs.insert(*txid, (tx.clone(), chain_position.cloned()));
            }
        }

        CanonicalView::new(self.tip, view_order, view_txs, self.spends.clone())
    }
}
