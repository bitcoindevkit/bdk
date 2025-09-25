use crate::collections::{HashMap, HashSet, VecDeque};
use crate::tx_graph::{TxAncestors, TxDescendants};
use crate::{Anchor, CanonicalView, ChainPosition, TxGraph};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::{BlockId, ChainQuery, ChainRequest, ChainResponse};
use bitcoin::{Transaction, Txid};

type CanonicalMap<A> = HashMap<Txid, (Arc<Transaction>, CanonicalReason<A>)>;
type NotCanonicalSet = HashSet<Txid>;

/// Modifies the canonicalization algorithm.
#[derive(Debug, Default, Clone)]
pub struct CanonicalizationParams {
    /// Transactions that will supersede all other transactions.
    ///
    /// In case of conflicting transactions within `assume_canonical`, transactions that appear
    /// later in the list (have higher index) have precedence.
    pub assume_canonical: Vec<Txid>,
}

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

    pending_anchor_checks: VecDeque<(Txid, Arc<Transaction>, Vec<A>)>,

    // Store canonical transactions in order
    canonical_order: Vec<Txid>,

    // Track which transactions have confirmed anchors
    confirmed_anchors: HashMap<Txid, A>,
}

impl<'g, A: Anchor> ChainQuery for CanonicalizationTask<'g, A> {
    type Output = CanonicalView<A>;

    fn next_query(&mut self) -> Option<ChainRequest> {
        // Check if we have pending anchor checks
        if let Some((_, _, anchors)) = self.pending_anchor_checks.front() {
            // Convert anchors to BlockIds for the ChainRequest
            let block_ids = anchors.iter().map(|anchor| anchor.anchor_block()).collect();
            return Some(ChainRequest {
                chain_tip: self.chain_tip,
                block_ids,
            });
        }

        // Process more anchored transactions if available
        self.process_anchored_txs()
    }

    fn resolve_query(&mut self, response: ChainResponse) {
        if let Some((txid, tx, anchors)) = self.pending_anchor_checks.pop_front() {
            // Find the anchor that matches the confirmed BlockId
            let best_anchor = response.and_then(|block_id| {
                anchors
                    .iter()
                    .find(|anchor| anchor.anchor_block() == block_id)
                    .cloned()
            });

            match best_anchor {
                Some(best_anchor) => {
                    self.confirmed_anchors.insert(txid, best_anchor.clone());
                    if !self.is_canonicalized(txid) {
                        self.mark_canonical(txid, tx, CanonicalReason::from_anchor(best_anchor));
                    }
                }
                None => {
                    self.unprocessed_leftover_txs.push_back((
                        txid,
                        tx,
                        anchors
                            .iter()
                            .last()
                            .expect(
                                "tx taken from `unprocessed_txs_with_anchors` so it must at least have an anchor",
                            )
                            .confirmation_height_upper_bound(),
                    ))
                }
            }
        }
    }

    fn is_finished(&mut self) -> bool {
        self.pending_anchor_checks.is_empty() && self.unprocessed_anchored_txs.size_hint().0 == 0
    }

    fn finish(mut self) -> Self::Output {
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

        CanonicalView::new(self.chain_tip, view_order, view_txs, view_spends)
    }
}

impl<'g, A: Anchor> CanonicalizationTask<'g, A> {
    /// Creates a new canonicalization task.
    pub fn new(
        tx_graph: &'g TxGraph<A>,
        chain_tip: BlockId,
        params: CanonicalizationParams,
    ) -> Self {
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

        // process assumed transactions first (they don't need queries)
        task.process_assumed_txs();

        task
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

    fn process_anchored_txs(&mut self) -> Option<ChainRequest> {
        while let Some((txid, tx, anchors)) = self.unprocessed_anchored_txs.next() {
            if !self.is_canonicalized(txid) {
                self.pending_anchor_checks
                    .push_back((txid, tx, anchors.iter().cloned().collect()));
                return self.next_query();
            }
        }
        None
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
            for (txid, tx, reason) in &staged_canonical {
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
                        // only check anchors we haven't already confirmed
                        if !self.confirmed_anchors.contains_key(txid) {
                            self.pending_anchor_checks.push_back((
                                *txid,
                                tx.clone(),
                                anchors.iter().cloned().collect(),
                            ));
                        }
                    }
                }
            }
        }
    }
}

/// Represents when and where a transaction was last observed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObservedIn {
    /// The transaction was last observed in a block of height.
    Block(u32),
    /// The transaction was last observed in the mempool at the given unix timestamp.
    Mempool(u64),
}

/// The reason why a transaction is canonical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalReason<A> {
    /// This transaction is explicitly assumed to be canonical by the caller, superceding all other
    /// canonicalization rules.
    Assumed {
        /// Whether it is a descendant that is assumed to be canonical.
        descendant: Option<Txid>,
    },
    /// This transaction is anchored in the best chain by `A`, and therefore canonical.
    Anchor {
        /// The anchor that anchored the transaction in the chain.
        anchor: A,
        /// Whether the anchor is of the transaction's descendant.
        descendant: Option<Txid>,
    },
    /// This transaction does not conflict with any other transaction with a more recent
    /// [`ObservedIn`] value or one that is anchored in the best chain.
    ObservedIn {
        /// The [`ObservedIn`] value of the transaction.
        observed_in: ObservedIn,
        /// Whether the [`ObservedIn`] value is of the transaction's descendant.
        descendant: Option<Txid>,
    },
}

impl<A: Clone> CanonicalReason<A> {
    /// Constructs a [`CanonicalReason`] for a transaction that is assumed to supercede all other
    /// transactions.
    pub fn assumed() -> Self {
        Self::Assumed { descendant: None }
    }

    /// Constructs a [`CanonicalReason`] from an `anchor`.
    pub fn from_anchor(anchor: A) -> Self {
        Self::Anchor {
            anchor,
            descendant: None,
        }
    }

    /// Constructs a [`CanonicalReason`] from an `observed_in` value.
    pub fn from_observed_in(observed_in: ObservedIn) -> Self {
        Self::ObservedIn {
            observed_in,
            descendant: None,
        }
    }

    /// Contruct a new [`CanonicalReason`] from the original which is transitive to `descendant`.
    ///
    /// This signals that either the [`ObservedIn`] or [`Anchor`] value belongs to the transaction's
    /// descendant, but is transitively relevant.
    pub fn to_transitive(&self, descendant: Txid) -> Self {
        match self {
            CanonicalReason::Assumed { .. } => Self::Assumed {
                descendant: Some(descendant),
            },
            CanonicalReason::Anchor { anchor, .. } => Self::Anchor {
                anchor: anchor.clone(),
                descendant: Some(descendant),
            },
            CanonicalReason::ObservedIn { observed_in, .. } => Self::ObservedIn {
                observed_in: *observed_in,
                descendant: Some(descendant),
            },
        }
    }

    /// This signals that either the [`ObservedIn`] or [`Anchor`] value belongs to the transaction's
    /// descendant.
    pub fn descendant(&self) -> &Option<Txid> {
        match self {
            CanonicalReason::Assumed { descendant, .. } => descendant,
            CanonicalReason::Anchor { descendant, .. } => descendant,
            CanonicalReason::ObservedIn { descendant, .. } => descendant,
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

        // Create canonicalization task and canonicalize using the chain
        let params = CanonicalizationParams::default();
        let task = CanonicalizationTask::new(&tx_graph, chain_tip, params);
        let canonical_view = chain.canonicalize(task);

        // Should have one canonical transaction
        assert_eq!(canonical_view.txs().len(), 1);
        let canon_tx = canonical_view.txs().next().unwrap();
        assert_eq!(canon_tx.txid, txid);
        assert_eq!(canon_tx.tx.compute_txid(), txid);

        // Should be confirmed (anchored)
        assert!(matches!(canon_tx.pos, ChainPosition::Confirmed { .. }));
    }
}
