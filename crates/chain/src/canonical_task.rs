use crate::collections::{HashMap, HashSet, VecDeque};
use crate::tx_graph::{TxAncestors, TxDescendants};
use crate::{canonical::CanonicalEntry, Anchor, CanonicalTxs, TxGraph};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::{
    BlockCandidateResolution, BlockId, BlockQueries, ChainTask, TaskProgress, ToBlockHash,
};
use bitcoin::{Transaction, Txid};

type CanonicalMap<A> = HashMap<Txid, CanonicalEntry<CanonicalReason<A>>>;
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
    fn next_stage(&mut self) {
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
    pub assume_canonical: Vec<Txid>,
}

/// Determines which transactions are canonical without resolving chain positions.
///
/// This task implements the first phase of canonicalization: it walks the transaction
/// graph and determines which transactions are canonical (non-conflicting) and why
/// (via [`CanonicalReason`](crate::CanonicalReason)). The output is a [`CanonicalTxs`] which can
/// then be further processed by [`CanonicalViewTask`](crate::CanonicalViewTask) to resolve reasons
/// into [`ChainPosition`](crate::ChainPosition)s.
pub struct CanonicalTask<'g, A, B> {
    tx_graph: &'g TxGraph<A>,

    queries: BlockQueries<B>,

    unprocessed_assumed_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>)> + 'g>,
    unprocessed_anchored_txs: VecDeque<(Txid, Arc<Transaction>, &'g BTreeSet<A>)>,
    unprocessed_seen_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>, u64)> + 'g>,
    unprocessed_leftover_txs: VecDeque<(Txid, Arc<Transaction>, u32)>,

    /// Whether the initial set of anchored leaves has been requested.
    requested_initial_leaves: bool,
    /// Anchored txs taken off the queue without sweeping their ancestors (their anchor is not in
    /// the best chain, or they are not canonical) since the last `Query`. Their anchored ancestors
    /// may have become leaves.
    exposing_txs: Vec<Arc<Transaction>>,

    canonical: CanonicalMap<A>,
    not_canonical: NotCanonicalSet,

    // Store canonical transactions in order
    canonical_order: Vec<Txid>,

    // Track the current stage of processing
    current_stage: CanonicalStage,
}

impl<'g, A: Anchor, B: ToBlockHash> ChainTask<B> for CanonicalTask<'g, A, B> {
    type Output = (CanonicalTxs<A>, BlockQueries<B>);

    fn tip(&self) -> BlockId {
        self.queries.tip()
    }

    fn unresolved_queries<'a>(&'a self) -> impl Iterator<Item = u32> + 'a {
        self.queries.unresolved()
    }

    fn poll(&mut self) -> TaskProgress {
        match self.current_stage {
            CanonicalStage::AssumedTxs => {
                if let Some((txid, tx)) = self.unprocessed_assumed_txs.next() {
                    if !self.is_canonicalized(txid) {
                        self.mark_canonical(txid, tx, CanonicalReason::assumed());
                    }
                } else {
                    self.current_stage.next_stage();
                }
                TaskProgress::Advanced
            }
            CanonicalStage::AnchoredTxs => {
                // Anchored txs are processed latest-first, and marking one canonical sweeps all its
                // ancestors transitively (see `mark_canonical`) without any further queries. So we
                // never query heights for every anchored tx, only for the *anchored leaves*: the
                // undecided anchored txs that are not ancestors of any other undecided anchored tx.
                //
                // Whenever the front tx needs block data, we also request the anchor heights of
                // every anchored leaf, so a round-trip driver fetches them in one batch instead of
                // one round-trip per tx. This only changes *when* heights are fetched: txs are
                // still taken in the same order below, so conflict resolution is unchanged.
                //
                // A leaf whose anchor is not in the best chain (stale anchor / reorg) does not
                // sweep its ancestors. Once it is deferred to the leftover stage, its nearest
                // anchored ancestors are batched in the next `Query`. Without reorgs, all anchored
                // txs are decided with a single `Query`.
                if let Some((txid, tx, anchors)) = self.unprocessed_anchored_txs.pop_front() {
                    if self.is_canonicalized(txid) {
                        if self.not_canonical.contains(&txid) {
                            self.exposing_txs.push(tx);
                        }
                        return TaskProgress::Advanced;
                    }

                    match self
                        .queries
                        .resolve_candidates(anchors.iter().map(|a| (a, a.anchor_block())))
                    {
                        BlockCandidateResolution::Confirmed(a) => {
                            self.mark_canonical(txid, tx, CanonicalReason::from_anchor(a.clone()));
                        }
                        BlockCandidateResolution::Query(mut heights) => {
                            self.unprocessed_anchored_txs
                                .push_front((txid, tx, anchors));
                            // `heights` is already marked as requested, so this only adds new
                            // heights and the batch stays deduplicated.
                            heights.extend(self.request_anchored_leaf_heights());
                            return TaskProgress::Query(heights);
                        }
                        BlockCandidateResolution::Awaiting => {
                            self.unprocessed_anchored_txs
                                .push_front((txid, tx, anchors));
                            return TaskProgress::Blocked;
                        }
                        BlockCandidateResolution::NotConfirmed => {
                            // No anchor confirms this tx; defer to the leftover stage.
                            self.exposing_txs.push(tx.clone());
                            self.unprocessed_leftover_txs.push_back((
                                txid,
                                tx,
                                anchors
                                    .iter()
                                    .last()
                                    .expect("must have at least one anchor")
                                    .anchor_block()
                                    .height,
                            ));
                        }
                    }
                } else {
                    self.current_stage.next_stage();
                }
                TaskProgress::Advanced
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
                } else {
                    self.current_stage.next_stage();
                }
                TaskProgress::Advanced
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
                } else {
                    self.current_stage.next_stage();
                }
                TaskProgress::Advanced
            }
            CanonicalStage::Finished => TaskProgress::Done,
        }
    }

    fn resolve_query(&mut self, height: u32, block: Option<B>) {
        self.queries.resolve(height, block);
    }

    fn finish(self) -> Self::Output {
        let mut view_spends = HashMap::new();

        for txid in &self.canonical_order {
            if let Some(entry) = self.canonical.get(txid) {
                if !entry.tx.is_coinbase() {
                    for input in &entry.tx.input {
                        view_spends.insert(input.previous_output, *txid);
                    }
                }
            }
        }

        (
            CanonicalTxs::new(
                self.queries.tip(),
                self.canonical_order,
                self.canonical,
                view_spends,
                None,
            ),
            self.queries,
        )
    }
}

impl<'g, A: Anchor, B: ToBlockHash> CanonicalTask<'g, A, B> {
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

            unprocessed_assumed_txs,
            unprocessed_anchored_txs,
            unprocessed_seen_txs,
            unprocessed_leftover_txs: VecDeque::new(),

            requested_initial_leaves: false,
            exposing_txs: Vec::new(),

            canonical: HashMap::new(),
            not_canonical: HashSet::new(),

            canonical_order: Vec::new(),
            current_stage: CanonicalStage::default(),

            queries: BlockQueries::new(chain_tip),
        }
    }

    fn is_canonicalized(&self, txid: Txid) -> bool {
        self.canonical.contains_key(&txid) || self.not_canonical.contains(&txid)
    }

    /// Requests the anchor heights of the anchored leaves and returns the newly requested ones.
    ///
    /// An anchored leaf is an anchored tx, not yet canonicalized, that is not an ancestor of any
    /// other such tx. Confirming a leaf sweeps its whole ancestor set, so these are the only
    /// heights we can need before any other anchored tx is decided.
    ///
    /// The first call finds the exact leaf set. Later calls only look at the nearest anchored
    /// ancestors of the txs that failed to sweep theirs since the previous call, so each call
    /// costs time proportional to what it uncovers rather than to the size of the graph.
    fn request_anchored_leaf_heights(&mut self) -> Vec<u32> {
        let leaves = if self.requested_initial_leaves {
            self.newly_exposed_anchored_txs()
        } else {
            self.requested_initial_leaves = true;
            // The initial leaf set already accounts for everything taken off the queue so far.
            self.exposing_txs.clear();
            self.initial_anchored_leaves()
        };
        let anchors = self.tx_graph.all_anchors();
        let heights = leaves
            .iter()
            .filter_map(|txid| anchors.get(txid))
            .flatten()
            .map(|anchor| anchor.anchor_block().height);
        self.queries.request(heights)
    }

    /// Returns the undecided anchored txs that are not ancestors of any other undecided anchored
    /// tx.
    fn initial_anchored_leaves(&self) -> Vec<Txid> {
        let undecided = self
            .unprocessed_anchored_txs
            .iter()
            .filter(|(txid, _, _)| !self.is_canonicalized(*txid))
            .collect::<Vec<_>>();

        // Every strict ancestor of an undecided anchored tx is not a leaf.
        let mut non_leaves = HashSet::<Txid>::new();
        TxAncestors::from_multiple_exclude_root(
            self.tx_graph,
            undecided.iter().map(|(_, tx, _)| tx.clone()),
            |_: usize, tx: Arc<Transaction>| -> Option<()> {
                let txid = tx.compute_txid();
                // A canonical tx's ancestors are all canonical too (`mark_canonical` sweeps
                // them), so none of them are undecided and we can stop walking here.
                if self.canonical.contains_key(&txid) {
                    return None;
                }
                non_leaves.insert(txid);
                Some(())
            },
        )
        .run_until_finished();

        undecided
            .into_iter()
            .map(|(txid, _, _)| *txid)
            .filter(|txid| !non_leaves.contains(txid))
            .collect()
    }

    /// Takes the txs in `exposing_txs` and returns their nearest undecided anchored ancestors.
    ///
    /// These are the only txs that can have become leaves since the last call. One of them may
    /// still be the ancestor of another undecided anchored tx, in which case its height may be
    /// fetched without being needed. That costs an extra fetch, never a wrong result.
    fn newly_exposed_anchored_txs(&mut self) -> Vec<Txid> {
        let anchors = self.tx_graph.all_anchors();
        let mut exposed = Vec::new();
        TxAncestors::from_multiple_exclude_root(
            self.tx_graph,
            core::mem::take(&mut self.exposing_txs),
            |_: usize, tx: Arc<Transaction>| -> Option<()> {
                let txid = tx.compute_txid();
                if self.canonical.contains_key(&txid) {
                    return None;
                }
                // Stop at the nearest anchored ancestor: whatever is behind it is covered by it.
                if anchors.contains_key(&txid) && !self.not_canonical.contains(&txid) {
                    exposed.push(txid);
                    return None;
                }
                Some(())
            },
        )
        .run_until_finished();
        exposed
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
                    reason.to_transitive(starting_txid)
                };

                use crate::collections::hash_map::Entry;
                let canonical_entry = match self.canonical.entry(this_txid) {
                    // Already visited tx before, exit early.
                    Entry::Occupied(_) => return None,
                    Entry::Vacant(entry) => entry,
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
                    detected_self_double_spend = true;
                    return None;
                }

                staged_canonical.push((this_txid, tx.clone(), this_reason.clone()));
                canonical_entry.insert(CanonicalEntry {
                    tx: tx.clone(),
                    pos: this_reason,
                    prev_mtp: None,
                });
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
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::local_chain::LocalChain;
    use crate::ChainPosition;
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

        // Create canonicalization task and canonicalize using the two-step pipeline
        let params = CanonicalParams::default();
        let task = CanonicalTask::new(&tx_graph, chain_tip, params);
        let output = chain.run_task(task);
        // `CanonicalTask::Output` is `(CanonicalTxs, BlockQueries)`. Both halves must be
        // `Debug` or callers cannot `dbg!()` it, or `.unwrap()` it out of a `Result`.
        let _ = alloc::format!("{output:?}");
        let (txs, queries) = output;
        let view_task = txs.view_task(&tx_graph, queries);
        let canonical_view = chain.run_task(view_task);

        // Should have one canonical transaction
        assert_eq!(canonical_view.txs().len(), 1);
        let canon_tx = canonical_view.txs().next().unwrap();
        assert_eq!(canon_tx.txid, txid);
        assert_eq!(canon_tx.tx.compute_txid(), txid);

        // Should be confirmed (anchored)
        assert!(matches!(canon_tx.pos, ChainPosition::Confirmed { .. }));
    }

    /// A chain of blocks `0..=10`, where block `h` has hash `[h; 32]`.
    fn test_chain() -> LocalChain {
        LocalChain::from_blocks((0..=10).map(|h| (h, block_hash(h as u8))).collect()).unwrap()
    }

    fn block_hash(n: u8) -> BlockHash {
        BlockHash::from_byte_array([n; 32])
    }

    /// The anchor of block `height` in [`test_chain`].
    fn anchor_in_chain(height: u32) -> BlockId {
        BlockId {
            height,
            hash: block_hash(height as u8),
        }
    }

    /// An anchor at `height` whose block is not in [`test_chain`] (e.g. reorged out).
    fn stale_anchor(height: u32) -> BlockId {
        BlockId {
            height,
            hash: block_hash(0xff),
        }
    }

    /// Builds a tx spending output 0 of each of `parents`. A tx with no parents spends a bogus
    /// outpoint. `tag` makes the txid unique.
    fn new_tx(parents: &[&Transaction], tag: u8) -> Transaction {
        let input = if parents.is_empty() {
            vec![TxIn {
                previous_output: bitcoin::OutPoint::new(Txid::from_byte_array([tag; 32]), 0),
                ..Default::default()
            }]
        } else {
            parents
                .iter()
                .map(|parent| TxIn {
                    previous_output: bitcoin::OutPoint::new(parent.compute_txid(), 0),
                    ..Default::default()
                })
                .collect()
        };
        bitcoin::Transaction {
            version: bitcoin::transaction::Version::ONE,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input,
            output: vec![TxOut {
                value: bitcoin::Amount::from_sat(1000 + tag as u64),
                script_pubkey: bitcoin::ScriptBuf::new(),
            }],
        }
    }

    /// Drives a [`CanonicalTask`] against `chain`, recording every [`TaskProgress::Query`] batch
    /// (sorted) in the order the task issued them.
    fn run_recording_queries(
        chain: &LocalChain,
        mut task: CanonicalTask<'_, BlockId, BlockHash>,
    ) -> (CanonicalTxs<BlockId>, Vec<Vec<u32>>) {
        let mut batches = Vec::new();
        loop {
            match task.poll() {
                TaskProgress::Advanced => continue,
                TaskProgress::Done => return (task.finish().0, batches),
                TaskProgress::Query(mut heights) => {
                    for &height in &heights {
                        task.resolve_query(height, chain.get(height).map(|cp| cp.hash()));
                    }
                    heights.sort_unstable();
                    batches.push(heights);
                }
                TaskProgress::Blocked => panic!("every queried height was resolved"),
            }
        }
    }

    fn reason_of(txs: &CanonicalTxs<BlockId>, tx: &Transaction) -> CanonicalReason<BlockId> {
        txs.tx(tx.compute_txid()).expect("tx must be canonical").pos
    }

    #[test]
    fn independent_anchored_leaves_are_queried_in_one_batch() {
        let chain = test_chain();
        let mut tx_graph = TxGraph::<BlockId>::default();
        let txs = [new_tx(&[], 1), new_tx(&[], 2), new_tx(&[], 3)];
        for (tx, height) in txs.iter().zip([1, 2, 3]) {
            let _ = tx_graph.insert_tx(tx.clone());
            let _ = tx_graph.insert_anchor(tx.compute_txid(), anchor_in_chain(height));
        }

        let task = CanonicalTask::new(&tx_graph, chain.tip().block_id(), Default::default());
        let (canonical, batches) = run_recording_queries(&chain, task);

        assert_eq!(batches, vec![vec![1, 2, 3]]);
        for (tx, height) in txs.iter().zip([1, 2, 3]) {
            assert_eq!(
                reason_of(&canonical, tx),
                CanonicalReason::from_anchor(anchor_in_chain(height))
            );
        }
    }

    #[test]
    fn anchored_ancestors_of_a_leaf_are_not_queried() {
        // tx_a <- tx_b <- tx_c, all anchored. Only tx_c is a leaf.
        let chain = test_chain();
        let mut tx_graph = TxGraph::<BlockId>::default();
        let tx_a = new_tx(&[], 1);
        let tx_b = new_tx(&[&tx_a], 2);
        let tx_c = new_tx(&[&tx_b], 3);
        for (tx, height) in [(&tx_a, 1), (&tx_b, 2), (&tx_c, 3)] {
            let _ = tx_graph.insert_tx(tx.clone());
            let _ = tx_graph.insert_anchor(tx.compute_txid(), anchor_in_chain(height));
        }

        let task = CanonicalTask::new(&tx_graph, chain.tip().block_id(), Default::default());
        let (canonical, batches) = run_recording_queries(&chain, task);

        assert_eq!(batches, vec![vec![3]]);
        let c_reason = CanonicalReason::from_anchor(anchor_in_chain(3));
        assert_eq!(reason_of(&canonical, &tx_c), c_reason);
        assert_eq!(
            reason_of(&canonical, &tx_b),
            c_reason.to_transitive(tx_c.compute_txid())
        );
        assert_eq!(
            reason_of(&canonical, &tx_a),
            c_reason.to_transitive(tx_c.compute_txid())
        );
    }

    #[test]
    fn stale_leaf_anchor_batches_its_parents_next() {
        // tx_p1 and tx_p2 are both spent by tx_child, whose anchor is stale. tx_other is an
        // independent anchored leaf.
        let chain = test_chain();
        let mut tx_graph = TxGraph::<BlockId>::default();
        let tx_p1 = new_tx(&[], 1);
        let tx_p2 = new_tx(&[], 2);
        let tx_child = new_tx(&[&tx_p1, &tx_p2], 3);
        let tx_other = new_tx(&[], 4);
        for (tx, anchor) in [
            (&tx_p1, anchor_in_chain(1)),
            (&tx_p2, anchor_in_chain(2)),
            (&tx_child, stale_anchor(5)),
            (&tx_other, anchor_in_chain(4)),
        ] {
            let _ = tx_graph.insert_tx(tx.clone());
            let _ = tx_graph.insert_anchor(tx.compute_txid(), anchor);
        }

        let task = CanonicalTask::new(&tx_graph, chain.tip().block_id(), Default::default());
        let (canonical, batches) = run_recording_queries(&chain, task);

        // First the leaves (tx_child, tx_other), then the parents exposed by the stale anchor,
        // together in one batch.
        assert_eq!(batches, vec![vec![4, 5], vec![1, 2]]);
        assert_eq!(
            reason_of(&canonical, &tx_p1),
            CanonicalReason::from_anchor(anchor_in_chain(1))
        );
        assert_eq!(
            reason_of(&canonical, &tx_p2),
            CanonicalReason::from_anchor(anchor_in_chain(2))
        );
        assert_eq!(
            reason_of(&canonical, &tx_other),
            CanonicalReason::from_anchor(anchor_in_chain(4))
        );
        // tx_child is only canonical via the leftover stage, by its stale anchor's height.
        assert_eq!(
            reason_of(&canonical, &tx_child),
            CanonicalReason::from_observed_in(ObservedIn::Block(5))
        );
    }

    #[test]
    fn not_canonical_tx_batches_its_parents_next() {
        // tx_x and tx_y double-spend tx_root. tx_x has the higher anchor and is in the best chain,
        // so it wins and tx_y becomes not canonical. tx_y's anchored parents tx_p1 and tx_p2 are
        // then no longer covered by it, so they are batched together.
        let chain = test_chain();
        let mut tx_graph = TxGraph::<BlockId>::default();
        let tx_root = new_tx(&[], 1);
        let tx_p1 = new_tx(&[], 2);
        let tx_p2 = new_tx(&[], 3);
        let tx_x = new_tx(&[&tx_root], 4);
        let tx_y = new_tx(&[&tx_root, &tx_p1, &tx_p2], 5);
        let _ = tx_graph.insert_tx(tx_root.clone());
        for (tx, height) in [(&tx_p1, 2), (&tx_p2, 3), (&tx_y, 5), (&tx_x, 6)] {
            let _ = tx_graph.insert_tx(tx.clone());
            let _ = tx_graph.insert_anchor(tx.compute_txid(), anchor_in_chain(height));
        }

        let task = CanonicalTask::new(&tx_graph, chain.tip().block_id(), Default::default());
        let (canonical, batches) = run_recording_queries(&chain, task);

        assert_eq!(batches, vec![vec![5, 6], vec![2, 3]]);
        assert_eq!(
            reason_of(&canonical, &tx_x),
            CanonicalReason::from_anchor(anchor_in_chain(6))
        );
        assert!(canonical.tx(tx_y.compute_txid()).is_none());
        assert_eq!(
            reason_of(&canonical, &tx_p1),
            CanonicalReason::from_anchor(anchor_in_chain(2))
        );
        assert_eq!(
            reason_of(&canonical, &tx_p2),
            CanonicalReason::from_anchor(anchor_in_chain(3))
        );
    }

    #[test]
    fn batching_does_not_change_conflict_resolution() {
        // tx_x and tx_y double-spend tx_root. tx_y has the higher anchor, so it is processed
        // first, but its anchor is stale; tx_x is anchored in the best chain and must win.
        let chain = test_chain();
        let mut tx_graph = TxGraph::<BlockId>::default();
        let tx_root = new_tx(&[], 1);
        let tx_x = new_tx(&[&tx_root], 2);
        let tx_y = new_tx(&[&tx_root], 3);
        let _ = tx_graph.insert_tx(tx_root.clone());
        for (tx, anchor) in [(&tx_x, anchor_in_chain(2)), (&tx_y, stale_anchor(3))] {
            let _ = tx_graph.insert_tx(tx.clone());
            let _ = tx_graph.insert_anchor(tx.compute_txid(), anchor);
        }

        let task = CanonicalTask::new(&tx_graph, chain.tip().block_id(), Default::default());
        let (canonical, batches) = run_recording_queries(&chain, task);

        assert_eq!(batches, vec![vec![2, 3]]);
        assert_eq!(
            reason_of(&canonical, &tx_x),
            CanonicalReason::from_anchor(anchor_in_chain(2))
        );
        assert!(canonical.tx(tx_y.compute_txid()).is_none());
    }
}
