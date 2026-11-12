use crate::collections::{HashMap, HashSet, VecDeque};
use crate::tx_graph::{TxAncestors, TxDescendants};
use crate::validation::TxValidity;
use crate::{Anchor, CanonicalTxs, TxGraph};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::{BlockId, ChainQuery, ChainRequest, ChainResponse};
use bitcoin::{Transaction, Txid};

type CanonicalMap<A> = HashMap<Txid, (Arc<Transaction>, CanonicalReason<A>)>;
type NotCanonicalSet = HashSet<Txid>;

/// Represents the current stage of canonicalization processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CanonicalStage {
    /// Processing transactions assumed to be canonical.
    #[default]
    AssumedTxs,
    /// Processing directly anchored transactions.
    AnchoredTxs,
    /// Processing transactions seen in mempool.
    SeenTxs,
    /// Processing leftover transactions.
    LeftOverTxs,
    /// All processing is complete.
    Finished,
}

impl CanonicalStage {
    fn advance(&mut self) {
        *self = match self {
            CanonicalStage::AssumedTxs => Self::AnchoredTxs,
            CanonicalStage::AnchoredTxs => Self::SeenTxs,
            CanonicalStage::SeenTxs => Self::LeftOverTxs,
            CanonicalStage::LeftOverTxs => Self::Finished,
            CanonicalStage::Finished => Self::Finished,
        };
    }
}

/// Modifies the canonicalization algorithm.
#[derive(Debug, Default, Clone)]
pub struct CanonicalParams {
    /// Transactions that will supersede all other transactions.
    ///
    /// In case of conflicting transactions within `assume_canonical`, transactions that appear
    /// later in the list (have higher index) have precedence.
    ///
    /// This overrides script validation: these transactions and their ancestors are canonical
    /// even if [`TxGraph::validate_tx`](crate::TxGraph::validate_tx) finds them invalid.
    pub assume_canonical: Vec<Txid>,
}

/// Determines which transactions are canonical without resolving chain positions.
///
/// This task implements the first phase of canonicalization: it walks the transaction
/// graph and determines which transactions are canonical (non-conflicting) and why
/// (via [`CanonicalReason`](crate::CanonicalReason)). The output is a [`CanonicalTxs`] which can
/// then be further processed by [`CanonicalViewTask`](crate::CanonicalViewTask) to resolve reasons
/// into [`ChainPosition`](crate::ChainPosition)s.
///
/// For unconfirmed candidates, retained copies are checked in the graph's witness preference
/// order. A Valid copy is preferred over Unknown; only an all-Invalid set is rejected. The output
/// carries the selected whole copy, which may differ from [`TxGraph::get_tx`]. Anchored and assumed
/// transactions retain the graph's preferred copy without script checks.
pub struct CanonicalTask<'g, A> {
    tx_graph: &'g TxGraph<A>,
    chain_tip: BlockId,

    unprocessed_assumed_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>)> + 'g>,
    unprocessed_anchored_txs: VecDeque<(Txid, Arc<Transaction>, &'g BTreeSet<A>)>,
    unprocessed_seen_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>, u64)> + 'g>,
    unprocessed_leftover_txs: VecDeque<(Txid, Arc<Transaction>, u32)>,

    canonical: CanonicalMap<A>,
    not_canonical: NotCanonicalSet,
    /// The selected eligible copy, or None if all copies are Invalid. The immutable graph makes
    /// these results reusable even after the walk that computed them is undone.
    selected_copies: HashMap<Txid, Option<Arc<Transaction>>>,
    #[cfg(test)]
    validations: usize,

    // Store canonical transactions in order
    canonical_order: Vec<Txid>,

    // Track the current stage of processing
    current_stage: CanonicalStage,
}

impl<'g, A: Anchor> ChainQuery for CanonicalTask<'g, A> {
    type Output = CanonicalTxs<A>;

    fn tip(&self) -> BlockId {
        self.chain_tip
    }

    fn next_query(&mut self) -> Option<ChainRequest> {
        loop {
            match self.current_stage {
                CanonicalStage::AssumedTxs => {
                    if let Some((txid, tx)) = self.unprocessed_assumed_txs.next() {
                        if !self.is_canonicalized(txid) {
                            self.mark_canonical(txid, tx, CanonicalReason::assumed());
                        }
                        continue;
                    }
                }
                CanonicalStage::AnchoredTxs => {
                    if let Some((_txid, _, anchors)) = self.unprocessed_anchored_txs.front() {
                        let block_ids =
                            anchors.iter().map(|anchor| anchor.anchor_block()).collect();
                        return Some(block_ids);
                    }
                }
                CanonicalStage::SeenTxs => {
                    if let Some((txid, tx, last_seen)) = self.unprocessed_seen_txs.next() {
                        // A coinbase tx cannot be in the mempool, so a `last_seen` value on one is
                        // not evidence of it being canonical.
                        if !self.is_canonicalized(txid) && !tx.is_coinbase() {
                            let observed_in = ObservedIn::Mempool(last_seen);
                            self.mark_canonical(
                                txid,
                                tx,
                                CanonicalReason::from_observed_in(observed_in),
                            );
                        }
                        continue;
                    }
                }
                CanonicalStage::LeftOverTxs => {
                    if let Some((txid, tx, height)) = self.unprocessed_leftover_txs.pop_front() {
                        let is_evicted = self
                            .tx_graph
                            .get_tx_node(txid)
                            .expect("leftover transaction must exist")
                            .is_evicted();
                        if !self.is_canonicalized(txid) && !tx.is_coinbase() && !is_evicted {
                            let observed_in = ObservedIn::Block(height);
                            self.mark_canonical(
                                txid,
                                tx,
                                CanonicalReason::from_observed_in(observed_in),
                            );
                        }
                        continue;
                    }
                }
                CanonicalStage::Finished => return None,
            }

            self.current_stage.advance();
        }
    }

    fn resolve_query(&mut self, response: ChainResponse) {
        match self.current_stage {
            CanonicalStage::AnchoredTxs => {
                // Process directly anchored transaction response
                if let Some((txid, tx, anchors)) = self.unprocessed_anchored_txs.pop_front() {
                    // Find the anchor that matches the confirmed BlockId
                    let best_anchor = response.and_then(|block_id| {
                        anchors
                            .iter()
                            .find(|anchor| anchor.anchor_block() == block_id)
                            .cloned()
                    });

                    match best_anchor {
                        Some(best_anchor) => {
                            // Transaction has a confirmed anchor
                            if !self.is_canonicalized(txid) {
                                self.mark_canonical(
                                    txid,
                                    tx,
                                    CanonicalReason::from_anchor(best_anchor),
                                );
                            }
                        }
                        None => {
                            // No confirmed anchor found, add to leftover transactions for later
                            // processing
                            self.unprocessed_leftover_txs.push_back((
                                txid,
                                tx,
                                anchors
                                    .iter()
                                    .last()
                                    .expect(
                                        "tx taken from `unprocessed_anchored_txs` so it must have at least one anchor",
                                    )
                                    .anchor_block()
                                    .height,
                            ))
                        }
                    }
                }
            }
            CanonicalStage::AssumedTxs
            | CanonicalStage::SeenTxs
            | CanonicalStage::LeftOverTxs
            | CanonicalStage::Finished => {
                // These stages don't generate queries and shouldn't receive responses
                debug_assert!(
                    false,
                    "resolve_query called for stage {:?} which doesn't generate queries",
                    self.current_stage
                );
            }
        }
    }

    fn finish(self) -> Self::Output {
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

                view_txs.insert(*txid, (tx.clone(), reason.clone()));
            }
        }

        CanonicalTxs::new(self.chain_tip, view_order, view_txs, view_spends)
    }
}

impl<'g, A: Anchor> CanonicalTask<'g, A> {
    /// Creates a new canonicalization task.
    pub fn new(tx_graph: &'g TxGraph<A>, chain_tip: BlockId, params: CanonicalParams) -> Self {
        let anchors = tx_graph.all_anchors();
        let unprocessed_assumed_txs = Box::new(
            params
                .assume_canonical
                .into_iter()
                .rev()
                .filter_map(|txid| Some((txid, tx_graph.get_tx(txid)?))),
        );
        let unprocessed_anchored_txs: VecDeque<_> = tx_graph
            .txids_by_descending_anchor_height()
            .filter_map(|(_, txid)| Some((txid, tx_graph.get_tx(txid)?, anchors.get(&txid)?)))
            .collect();
        let unprocessed_seen_txs = Box::new(
            tx_graph
                .txids_by_descending_last_seen()
                .filter_map(|(last_seen, txid)| Some((txid, tx_graph.get_tx(txid)?, last_seen))),
        );

        Self {
            tx_graph,
            chain_tip,

            unprocessed_assumed_txs,
            unprocessed_anchored_txs,
            unprocessed_seen_txs,
            unprocessed_leftover_txs: VecDeque::new(),

            canonical: HashMap::new(),
            not_canonical: HashSet::new(),
            selected_copies: HashMap::new(),
            #[cfg(test)]
            validations: 0,

            canonical_order: Vec::new(),
            current_stage: CanonicalStage::default(),
        }
    }

    fn is_canonicalized(&self, txid: Txid) -> bool {
        self.canonical.contains_key(&txid) || self.not_canonical.contains(&txid)
    }

    fn mark_canonical(&mut self, txid: Txid, tx: Arc<Transaction>, reason: CanonicalReason<A>) {
        let starting_txid = txid;
        let mut is_starting_tx = true;

        // We keep track of changes made so far so that we can undo them if `tx` turns out not to be
        // canonical: it double spends itself, or it or an ancestor has invalid scripts.
        let mut rejected = false;
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
                    reason.to_transitive(starting_txid)
                };

                use crate::collections::hash_map::Entry;
                let canonical_entry = match self.canonical.entry(this_txid) {
                    // Already visited tx before, exit early.
                    Entry::Occupied(_) => return None,
                    Entry::Vacant(entry) => entry,
                };

                // Validation guards against acting on unconfirmed transactions that can never
                // confirm. An anchor in the best chain or an explicit assumption outranks it. An
                // unknown verdict does not bar it: the graph may just lack a parent.
                let tx = if this_reason.is_unconfirmed() {
                    let selected = self.selected_copies.entry(this_txid).or_insert_with(|| {
                        let mut copies = self.tx_graph.tx_copies(this_txid).collect::<Vec<_>>();
                        copies.sort_by_cached_key(|copy| {
                            (
                                crate::tx_graph::witness_preference(copy),
                                copy.compute_wtxid(),
                            )
                        });
                        let mut unknown = None;
                        for copy in copies {
                            #[cfg(test)]
                            {
                                self.validations += 1;
                            }
                            match self.tx_graph.validate_tx(&copy).validity() {
                                TxValidity::Valid => return Some(copy),
                                TxValidity::Unknown if unknown.is_none() => unknown = Some(copy),
                                TxValidity::Unknown | TxValidity::Invalid => {}
                            }
                        }
                        unknown
                    });
                    match selected {
                        Some(copy) => copy.clone(),
                        None => {
                            rejected = true;
                            return None;
                        }
                    }
                } else {
                    tx
                };

                // Prune conflicts
                //
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
                    rejected = true;
                    return None;
                }

                staged_canonical.push((this_txid, tx.clone(), this_reason.clone()));
                canonical_entry.insert((tx.clone(), this_reason));
                Some(this_txid)
            },
        )
        .run_until_finished();

        if rejected {
            // Undo changes
            for (txid, _, _) in staged_canonical {
                self.canonical.remove(&txid);
            }
            for txid in undo_not_canonical {
                self.not_canonical.remove(&txid);
            }
            return;
        }

        // Add to canonical order
        for (txid, _, _) in &staged_canonical {
            self.canonical_order.push(*txid);
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

    /// Construct a new [`CanonicalReason`] from the original which is transitive to `descendant`.
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

    /// Returns true if this reason represents a transitive canonicalization
    /// (i.e., the transaction is canonical because of its descendant).
    pub fn is_transitive(&self) -> bool {
        self.descendant().is_some()
    }

    /// Returns true if this reason is [`CanonicalReason::Assumed`].
    pub fn is_assumed(&self) -> bool {
        matches!(self, CanonicalReason::Assumed { .. })
    }

    /// Whether this reason leaves the transaction unconfirmed, which is when script validity
    /// decides it. Exhaustive so that a new reason has to make that choice.
    fn is_unconfirmed(&self) -> bool {
        match self {
            CanonicalReason::Assumed { .. } | CanonicalReason::Anchor { .. } => false,
            CanonicalReason::ObservedIn { .. } => true,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::local_chain::LocalChain;
    use crate::ChainPosition;
    use bitcoin::{hashes::Hash, opcodes, BlockHash, OutPoint, ScriptBuf, TxIn, TxOut};

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

        // Create canonicalization task and canonicalize using the two-step pipeline
        let params = CanonicalParams::default();
        let task = CanonicalTask::new(&tx_graph, chain_tip, params);
        let canonical_txs = chain.canonicalize(task);
        let view_task = canonical_txs.view_task(&tx_graph);
        let canonical_view = chain.canonicalize(view_task);

        // Should have one canonical transaction
        assert_eq!(canonical_view.txs().len(), 1);
        let canon_tx = canonical_view.txs().next().unwrap();
        assert_eq!(canon_tx.txid, txid);
        assert_eq!(canon_tx.tx.compute_txid(), txid);

        // Should be confirmed (anchored)
        assert!(matches!(canon_tx.pos, ChainPosition::Confirmed { .. }));
    }

    #[test]
    fn a_valid_ancestor_of_a_rejected_candidate_is_checked_once() {
        let chain = LocalChain::from_blocks([(0, BlockHash::all_zeros())].into()).unwrap();
        let anyone = ScriptBuf::from_bytes(vec![opcodes::OP_TRUE.to_u8()]);
        let unspendable = ScriptBuf::new_op_return([]);
        let txout = |script_pubkey: &ScriptBuf| TxOut {
            value: bitcoin::Amount::from_sat(1000),
            script_pubkey: script_pubkey.clone(),
        };
        let spend = |prevouts: &[OutPoint], outputs: usize| bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: prevouts
                .iter()
                .map(|&previous_output| TxIn {
                    previous_output,
                    ..TxIn::default()
                })
                .collect(),
            output: vec![txout(&anyone); outputs],
        };

        let mut tx_graph = TxGraph::<crate::ConfirmationBlockTime>::default();
        let anyone_coin = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
        let unspendable_coin = OutPoint::new(Txid::from_byte_array([2; 32]), 0);
        let _ = tx_graph.insert_txout(anyone_coin, txout(&anyone));
        let _ = tx_graph.insert_txout(unspendable_coin, txout(&unspendable));
        let shared = spend(&[anyone_coin], 2);
        let invalid = spend(&[unspendable_coin], 1);
        let rejected = spend(
            &[
                OutPoint::new(shared.compute_txid(), 0),
                OutPoint::new(invalid.compute_txid(), 0),
            ],
            1,
        );
        let later = spend(&[OutPoint::new(shared.compute_txid(), 1)], 1);
        for tx in [&shared, &invalid, &rejected, &later] {
            let _ = tx_graph.insert_tx(tx.clone());
        }
        let mut bogus_shared = shared.clone();
        bogus_shared.input[0].witness = bitcoin::Witness::from_slice(&[vec![0]]);
        let _ = tx_graph.insert_tx(bogus_shared);
        assert_ne!(*tx_graph.get_tx(shared.compute_txid()).unwrap(), shared);
        let _ = tx_graph.insert_seen_at(rejected.compute_txid(), 2);
        let _ = tx_graph.insert_seen_at(later.compute_txid(), 1);

        let mut task = CanonicalTask::new(
            &tx_graph,
            chain.tip().block_id(),
            CanonicalParams::default(),
        );
        assert!(task.next_query().is_none(), "nothing is anchored");

        assert_eq!(
            task.validations, 5,
            "each copy is checked once, including both copies of the shared ancestor"
        );
        assert_eq!(
            task.selected_copies[&shared.compute_txid()].as_deref(),
            Some(&shared)
        );
        let mut canonical = task.canonical.keys().copied().collect::<Vec<_>>();
        canonical.sort();
        let mut expected = vec![shared.compute_txid(), later.compute_txid()];
        expected.sort();
        assert_eq!(canonical, expected);
    }
}
