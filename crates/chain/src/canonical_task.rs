use crate::canonical_iter::{CanonicalReason, ObservedIn};
use crate::collections::{HashMap, HashSet, VecDeque};
use crate::tx_graph::{TxAncestors, TxDescendants};
use crate::{Anchor, CanonicalView, ChainPosition, TxGraph};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::BlockId;
use bitcoin::{Transaction, Txid};

/// A request for chain data needed during canonicalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalizationRequest {
    /// Request to check if a block is in the chain.
    IsBlockInChain {
        /// The block to check.
        block: BlockId,
        /// The chain tip to check against.
        chain_tip: BlockId,
    },
}

/// A response to a canonicalization request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalizationResponse {
    /// Response to IsBlockInChain request.
    IsBlockInChain(Option<bool>),
}

/// Parameters that modify the canonicalization algorithm.
pub use crate::canonical_iter::CanonicalizationParams;

type CanonicalMap<A> = HashMap<Txid, (Arc<Transaction>, CanonicalReason<A>)>;
type NotCanonicalSet = HashSet<Txid>;

/// Manages the canonicalization process without direct I/O operations.
pub struct CanonicalizationTask<'g, A> {
    tx_graph: &'g TxGraph<A>,
    chain_tip: BlockId,

    unprocessed_assumed_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>)> + 'g>,
    unprocessed_anchored_txs:
        Box<dyn Iterator<Item = (Txid, Arc<Transaction>, &'g BTreeSet<A>)> + 'g>,
    unprocessed_seen_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>, u64)> + 'g>,
    unprocessed_leftover_txs: VecDeque<(Txid, Arc<Transaction>, u32)>,

    canonical: CanonicalMap<A>,
    not_canonical: NotCanonicalSet,

    pending_anchor_checks: VecDeque<(Txid, Arc<Transaction>, Vec<A>, usize)>,

    // Store canonical transactions in order
    canonical_order: Vec<Txid>,

    // Track which transactions have confirmed anchors
    confirmed_anchors: HashMap<Txid, A>,
}

impl<'g, A: Anchor> CanonicalizationTask<'g, A> {
    /// Creates a new canonicalization task.
    ///
    /// Returns the task and an optional initial request.
    pub fn new(
        tx_graph: &'g TxGraph<A>,
        chain_tip: BlockId,
        params: CanonicalizationParams,
    ) -> (Self, Option<CanonicalizationRequest>) {
        let anchors = tx_graph.all_anchors();
        let unprocessed_assumed_txs = Box::new(
            params
                .assume_canonical
                .into_iter()
                .rev()
                .filter_map(|txid| Some((txid, tx_graph.get_tx(txid)?))),
        );
        let unprocessed_anchored_txs = Box::new(
            tx_graph
                .txids_by_descending_anchor_height()
                .filter_map(|(_, txid)| Some((txid, tx_graph.get_tx(txid)?, anchors.get(&txid)?))),
        );
        let unprocessed_seen_txs = Box::new(
            tx_graph
                .txids_by_descending_last_seen()
                .filter_map(|(last_seen, txid)| Some((txid, tx_graph.get_tx(txid)?, last_seen))),
        );

        let mut task = Self {
            tx_graph,
            chain_tip,
            unprocessed_assumed_txs,
            unprocessed_anchored_txs,
            unprocessed_seen_txs,
            unprocessed_leftover_txs: VecDeque::new(),
            canonical: HashMap::new(),
            not_canonical: HashSet::new(),
            pending_anchor_checks: VecDeque::new(),
            canonical_order: Vec::new(),
            confirmed_anchors: HashMap::new(),
        };

        // Process assumed transactions first (they don't need queries)
        task.process_assumed_txs();

        // Process anchored transactions and get the first request if needed
        let initial_request = task.process_next_anchored_batch();

        (task, initial_request)
    }

    /// Returns the next query needed, if any.
    pub fn next_query(&mut self) -> Option<CanonicalizationRequest> {
        // Check if we have pending anchor checks
        if let Some((_, _, anchors, idx)) = self.pending_anchor_checks.front() {
            if *idx < anchors.len() {
                let anchor = &anchors[*idx];
                return Some(CanonicalizationRequest::IsBlockInChain {
                    block: anchor.anchor_block(),
                    chain_tip: self.chain_tip,
                });
            }
        }

        // Process more anchored transactions if available
        self.process_next_anchored_batch()
    }

    /// Resolves a query with the given response.
    pub fn resolve_query(&mut self, response: CanonicalizationResponse) {
        match response {
            CanonicalizationResponse::IsBlockInChain(result) => {
                if let Some((txid, tx, anchors, idx)) = self.pending_anchor_checks.front_mut() {
                    if result == Some(true) && *idx < anchors.len() {
                        // This anchor is in the chain, mark transaction as canonical
                        let anchor = anchors[*idx].clone();
                        let txid = *txid;
                        let tx = tx.clone();

                        // Remove from pending checks
                        self.pending_anchor_checks.pop_front();

                        // Track this confirmed anchor
                        self.confirmed_anchors.insert(txid, anchor.clone());

                        // Check if this transaction was already marked canonical transitively
                        if let Some((_, reason)) = self.canonical.get(&txid) {
                            if matches!(
                                reason,
                                CanonicalReason::Anchor {
                                    descendant: Some(_),
                                    ..
                                }
                            ) {
                                // Update to direct anchor
                                if let Some((_, reason)) = self.canonical.get_mut(&txid) {
                                    *reason = CanonicalReason::from_anchor(anchor);
                                }
                            }
                        } else {
                            // Mark as canonical
                            self.mark_canonical(txid, tx, CanonicalReason::from_anchor(anchor));
                        }
                    } else {
                        // Move to next anchor
                        *idx += 1;

                        // If we've checked all anchors, move to leftover
                        if *idx >= anchors.len() {
                            let (txid, tx, anchors, _) =
                                self.pending_anchor_checks.pop_front().unwrap();
                            let height = anchors
                                .last()
                                .map(|a| a.confirmation_height_upper_bound())
                                .unwrap_or(0);
                            self.unprocessed_leftover_txs.push_back((txid, tx, height));
                        }
                    }
                }
            }
        }
    }

    /// Returns true if the canonicalization process is complete.
    pub fn is_finished(&self) -> bool {
        self.pending_anchor_checks.is_empty() && self.unprocessed_anchored_txs.size_hint().0 == 0
    }

    /// Completes the canonicalization and returns a CanonicalView.
    pub fn finish(mut self) -> CanonicalView<A> {
        // Process remaining transactions (seen and leftover)
        self.process_seen_txs();
        self.process_leftover_txs();

        // Build the canonical view
        let mut view_order = Vec::new();
        let mut view_txs = HashMap::new();
        let mut view_spends = HashMap::new();

        for txid in &self.canonical_order {
            if let Some((tx, reason)) = self.canonical.get(txid) {
                view_order.push(*txid);

                // Add spends
                if !tx.is_coinbase() {
                    for input in &tx.input {
                        view_spends.insert(input.previous_output, *txid);
                    }
                }

                // Get transaction node for first_seen/last_seen info
                // let tx_node = self.tx_graph.get_tx_node(*txid);
                let tx_node = match self.tx_graph.get_tx_node(*txid) {
                    Some(tx_node) => tx_node,
                    None => {
                        debug_assert!(false, "tx node must exist!");
                        continue;
                    }
                };

                // Determine chain position based on reason
                let chain_position = match reason {
                    CanonicalReason::Assumed { descendant } => match descendant {
                        Some(_) => match self.confirmed_anchors.get(txid) {
                            Some(anchor) => ChainPosition::Confirmed {
                                anchor,
                                transitively: None,
                            },
                            None => ChainPosition::Unconfirmed {
                                first_seen: tx_node.first_seen,
                                last_seen: tx_node.last_seen,
                            },
                        },
                        None => ChainPosition::Unconfirmed {
                            first_seen: tx_node.first_seen,
                            last_seen: tx_node.last_seen,
                        },
                    },
                    CanonicalReason::Anchor { anchor, descendant } => match descendant {
                        Some(_) => match self.confirmed_anchors.get(txid) {
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

        CanonicalView::from_parts(self.chain_tip, view_order, view_txs, view_spends)
    }

    fn process_next_anchored_batch(&mut self) -> Option<CanonicalizationRequest> {
        while let Some((txid, tx, anchors)) = self.unprocessed_anchored_txs.next() {
            if !self.is_canonicalized(txid) {
                // Check if we already have a confirmed anchor for this transaction
                if let Some(anchor) = self.confirmed_anchors.get(&txid).cloned() {
                    self.mark_canonical(txid, tx, CanonicalReason::from_anchor(anchor));
                } else if !anchors.is_empty() {
                    let anchors_vec: Vec<A> = anchors.iter().cloned().collect();
                    self.pending_anchor_checks
                        .push_back((txid, tx, anchors_vec, 0));
                    return self.next_query();
                }
            }
        }
        None
    }

    fn is_canonicalized(&self, txid: Txid) -> bool {
        self.canonical.contains_key(&txid) || self.not_canonical.contains(&txid)
    }

    fn process_assumed_txs(&mut self) {
        while let Some((txid, tx)) = self.unprocessed_assumed_txs.next() {
            if !self.is_canonicalized(txid) {
                self.mark_canonical(txid, tx, CanonicalReason::assumed());
            }
        }
    }

    fn process_seen_txs(&mut self) {
        while let Some((txid, tx, last_seen)) = self.unprocessed_seen_txs.next() {
            debug_assert!(
                !tx.is_coinbase(),
                "Coinbase txs must not have `last_seen` (in mempool) value"
            );
            if !self.is_canonicalized(txid) {
                let observed_in = ObservedIn::Mempool(last_seen);
                self.mark_canonical(txid, tx, CanonicalReason::from_observed_in(observed_in));
            }
        }
    }

    fn process_leftover_txs(&mut self) {
        while let Some((txid, tx, height)) = self.unprocessed_leftover_txs.pop_front() {
            if !self.is_canonicalized(txid) && !tx.is_coinbase() {
                let observed_in = ObservedIn::Block(height);
                self.mark_canonical(txid, tx, CanonicalReason::from_observed_in(observed_in));
            }
        }
    }

    fn mark_canonical(&mut self, txid: Txid, tx: Arc<Transaction>, reason: CanonicalReason<A>) {
        let starting_txid = txid;
        let mut is_starting_tx = true;

        // We keep track of changes made so far so that we can undo it later in case we detect that
        // `tx` double spends itself.
        let mut detected_self_double_spend = false;
        let mut undo_not_canonical = Vec::<Txid>::new();
        let mut staged_canonical = Vec::<(Txid, Arc<Transaction>, CanonicalReason<A>)>::new();

        // Process ancestors
        TxAncestors::new_include_root(
            self.tx_graph,
            tx,
            |_: usize, tx: Arc<Transaction>| -> Option<Txid> {
                let this_txid = tx.compute_txid();
                let this_reason = if is_starting_tx {
                    is_starting_tx = false;
                    reason.clone()
                } else {
                    // This is an ancestor being marked transitively
                    // Check if it has its own anchor that needs to be verified later
                    // We'll check anchors after marking it canonical
                    reason.to_transitive(starting_txid)
                };

                use crate::collections::hash_map::Entry;
                let canonical_entry = match self.canonical.entry(this_txid) {
                    // Already visited tx before, exit early.
                    Entry::Occupied(_) => return None,
                    Entry::Vacant(entry) => entry,
                };

                // Any conflicts with a canonical tx can be added to `not_canonical`. Descendants
                // of `not_canonical` txs can also be added to `not_canonical`.
                for (_, conflict_txid) in self.tx_graph.direct_conflicts(&tx) {
                    TxDescendants::new_include_root(
                        self.tx_graph,
                        conflict_txid,
                        |_: usize, txid: Txid| -> Option<()> {
                            if self.not_canonical.insert(txid) {
                                undo_not_canonical.push(txid);
                                Some(())
                            } else {
                                None
                            }
                        },
                    )
                    .run_until_finished()
                }

                if self.not_canonical.contains(&this_txid) {
                    // Early exit if self-double-spend is detected.
                    detected_self_double_spend = true;
                    return None;
                }

                staged_canonical.push((this_txid, tx.clone(), this_reason.clone()));
                canonical_entry.insert((tx.clone(), this_reason));
                Some(this_txid)
            },
        )
        .run_until_finished();

        if detected_self_double_spend {
            // Undo changes
            for (txid, _, _) in staged_canonical {
                self.canonical.remove(&txid);
            }
            for txid in undo_not_canonical {
                self.not_canonical.remove(&txid);
            }
        } else {
            // Add to canonical order
            for (txid, _, reason) in &staged_canonical {
                self.canonical_order.push(*txid);

                // If this was marked transitively, check if it has anchors to verify
                let is_transitive = matches!(
                    reason,
                    CanonicalReason::Anchor {
                        descendant: Some(_),
                        ..
                    } | CanonicalReason::Assumed {
                        descendant: Some(_),
                        ..
                    }
                );

                if is_transitive {
                    if let Some(anchors) = self.tx_graph.all_anchors().get(txid) {
                        // Only check anchors we haven't already confirmed
                        if !self.confirmed_anchors.contains_key(txid) && !anchors.is_empty() {
                            let tx = self.tx_graph.get_tx(*txid).expect("tx must exist");
                            let anchors_vec: Vec<A> = anchors.iter().cloned().collect();
                            self.pending_anchor_checks
                                .push_back((*txid, tx, anchors_vec, 0));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_chain::LocalChain;
    use bitcoin::{hashes::Hash, BlockHash, TxIn, TxOut};

    #[test]
    fn test_canonicalization_task_sans_io() {
        // Create a simple chain
        let blocks = [
            (0, BlockHash::all_zeros()),
            (1, BlockHash::from_byte_array([1; 32])),
            (2, BlockHash::from_byte_array([2; 32])),
        ];
        let chain = LocalChain::from_blocks(blocks.into_iter().collect()).unwrap();
        let chain_tip = chain.tip().block_id();

        // Create a simple transaction graph
        let mut tx_graph = TxGraph::default();

        // Add a transaction
        let tx = bitcoin::Transaction {
            version: bitcoin::transaction::Version::ONE,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn::default()],
            output: vec![TxOut {
                value: bitcoin::Amount::from_sat(1000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            }],
        };
        let _ = tx_graph.insert_tx(tx.clone());
        let txid = tx.compute_txid();

        // Add an anchor at height 1
        let anchor = crate::ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 12345,
        };
        let _ = tx_graph.insert_anchor(txid, anchor);

        // Create canonicalization task
        let params = CanonicalizationParams::default();
        let (mut task, initial_request) = CanonicalizationTask::new(&tx_graph, chain_tip, params);

        // Process requests
        if let Some(request) = initial_request {
            let response = chain.handle_canonicalization_request(&request).unwrap();
            task.resolve_query(response);
        }

        while let Some(request) = task.next_query() {
            let response = chain.handle_canonicalization_request(&request).unwrap();
            task.resolve_query(response);
        }

        // Get canonical view
        let canonical_view = task.finish();

        // Should have one canonical transaction
        assert_eq!(canonical_view.txs().len(), 1);
        let canon_tx = canonical_view.txs().next().unwrap();
        assert_eq!(canon_tx.txid, txid);
        assert_eq!(canon_tx.tx.compute_txid(), txid);

        // Should be confirmed (anchored)
        assert!(matches!(canon_tx.pos, ChainPosition::Confirmed { .. }));
    }
}
