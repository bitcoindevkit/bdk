use crate::collections::{HashMap, HashSet, VecDeque};
use crate::tx_graph::{CanonicalTx, TxAncestors, TxDescendants};
use crate::{Anchor, ChainOracle, TxGraph};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::BlockId;
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

/// Iterates over canonical txs.
pub struct CanonicalIter<'g, A, C> {
    tx_graph: &'g TxGraph<A>,
    chain: &'g C,
    chain_tip: BlockId,

    unprocessed_assumed_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>)> + 'g>,
    unprocessed_anchored_txs:
        Box<dyn Iterator<Item = (Txid, Arc<Transaction>, &'g BTreeSet<A>)> + 'g>,
    unprocessed_seen_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>, u64)> + 'g>,
    unprocessed_leftover_txs: VecDeque<(Txid, Arc<Transaction>, u32)>,

    canonical: CanonicalMap<A>,
    not_canonical: NotCanonicalSet,

    queue: VecDeque<Txid>,
}

impl<'g, A: Anchor, C: ChainOracle> CanonicalIter<'g, A, C> {
    /// Constructs [`CanonicalIter`].
    pub fn new(
        tx_graph: &'g TxGraph<A>,
        chain: &'g C,
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
        Self {
            tx_graph,
            chain,
            chain_tip,
            unprocessed_assumed_txs,
            unprocessed_anchored_txs,
            unprocessed_seen_txs,
            unprocessed_leftover_txs: VecDeque::new(),
            canonical: HashMap::new(),
            not_canonical: HashSet::new(),
            queue: VecDeque::new(),
        }
    }

    /// Whether this transaction is already canonicalized.
    fn is_canonicalized(&self, txid: Txid) -> bool {
        self.canonical.contains_key(&txid) || self.not_canonical.contains(&txid)
    }

    /// Mark transaction as canonical if it is anchored in the best chain.
    fn scan_anchors(
        &mut self,
        txid: Txid,
        tx: Arc<Transaction>,
        anchors: &BTreeSet<A>,
    ) -> Result<(), C::Error> {
        for anchor in anchors {
            let in_chain_opt = self
                .chain
                .is_block_in_chain(anchor.anchor_block(), self.chain_tip)?;
            if in_chain_opt == Some(true) {
                self.mark_canonical(txid, tx, CanonicalReason::from_anchor(anchor.clone()));
                return Ok(());
            }
        }
        // cannot determine
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
        ));
        Ok(())
    }

    /// Marks `tx` and it's ancestors as canonical and mark all conflicts of these as
    /// `not_canonical`.
    ///
    /// The exception is when it is discovered that `tx` double spends itself (i.e. two of it's
    /// inputs conflict with each other), then no changes will be made.
    ///
    /// The logic works by having two loops where one is nested in another.
    /// * The outer loop iterates through ancestors of `tx` (including `tx`). We can transitively
    ///   assume that all ancestors of `tx` are also canonical.
    /// * The inner loop loops through conflicts of ancestors of `tx`. Any descendants of conflicts
    ///   are also conflicts and are transitively considered non-canonical.
    ///
    /// If the inner loop ends up marking `tx` as non-canonical, then we know that it double spends
    /// itself.
    fn mark_canonical(&mut self, txid: Txid, tx: Arc<Transaction>, reason: CanonicalReason<A>) {
        let starting_txid = txid;
        let mut is_starting_tx = true;

        // We keep track of changes made so far so that we can undo it later in case we detect that
        // `tx` double spends itself.
        let mut detected_self_double_spend = false;
        let mut undo_not_canonical = Vec::<Txid>::new();

        // `staged_queue` doubles as the `undo_canonical` data.
        let staged_queue = TxAncestors::new_include_root(
            self.tx_graph,
            tx,
            |_: usize, tx: Arc<Transaction>| -> Option<Txid> {
                let this_txid = tx.compute_txid();
                let this_reason = if is_starting_tx {
                    is_starting_tx = false;
                    reason.clone()
                } else {
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
                canonical_entry.insert((tx, this_reason));
                Some(this_txid)
            },
        )
        .collect::<Vec<Txid>>();

        if detected_self_double_spend {
            for txid in staged_queue {
                self.canonical.remove(&txid);
            }
            for txid in undo_not_canonical {
                self.not_canonical.remove(&txid);
            }
        } else {
            self.queue.extend(staged_queue);
        }
    }
}

impl<A: Anchor, C: ChainOracle> Iterator for CanonicalIter<'_, A, C> {
    type Item = Result<(Txid, Arc<Transaction>, CanonicalReason<A>), C::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(txid) = self.queue.pop_front() {
                let (tx, reason) = self
                    .canonical
                    .get(&txid)
                    .cloned()
                    .expect("reason must exist");
                return Some(Ok((txid, tx, reason)));
            }

            if let Some((txid, tx)) = self.unprocessed_assumed_txs.next() {
                if !self.is_canonicalized(txid) {
                    self.mark_canonical(txid, tx, CanonicalReason::assumed());
                }
            }

            if let Some((txid, tx, anchors)) = self.unprocessed_anchored_txs.next() {
                if !self.is_canonicalized(txid) {
                    if let Err(err) = self.scan_anchors(txid, tx, anchors) {
                        return Some(Err(err));
                    }
                }
                continue;
            }

            if let Some((txid, tx, last_seen)) = self.unprocessed_seen_txs.next() {
                debug_assert!(
                    !tx.is_coinbase(),
                    "Coinbase txs must not have `last_seen` (in mempool) value"
                );
                if !self.is_canonicalized(txid) {
                    let observed_in = ObservedIn::Mempool(last_seen);
                    self.mark_canonical(txid, tx, CanonicalReason::from_observed_in(observed_in));
                }
                continue;
            }

            if let Some((txid, tx, height)) = self.unprocessed_leftover_txs.pop_front() {
                if !self.is_canonicalized(txid) && !tx.is_coinbase() {
                    let observed_in = ObservedIn::Block(height);
                    self.mark_canonical(txid, tx, CanonicalReason::from_observed_in(observed_in));
                }
                continue;
            }

            return None;
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
    /// This transaction is explicitly assumed to be canonical by the caller, superseding all other
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
    /// Constructs a [`CanonicalReason`] for a transaction that is assumed to supersede all other
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
}

/// Iterator based on the Kahn's Algorithm, that yields transactions in topological spending order
/// in depth, and properly sorted with level.
///
/// NOTE: Please refer to the Kahn's Algorithm reference: https://dl.acm.org/doi/pdf/10.1145/368996.369025
pub(crate) struct TopologicalIterator<'a, A> {
    /// Map of txid to its canonical transaction
    canonical_txs: HashMap<Txid, CanonicalTx<'a, Arc<Transaction>, A>>,

    /// Current level of transactions to process
    current_level: Vec<Txid>,
    /// Next level of transactions to process
    next_level: Vec<Txid>,

    /// Adjacency list: parent txid -> list of children txids
    children_map: HashMap<Txid, Vec<Txid>>,
    /// Number of unprocessed parents for each transaction
    parent_count: HashMap<Txid, usize>,

    /// Current index in the current level
    current_index: usize,
}

impl<'a, A: Clone + Anchor> TopologicalIterator<'a, A> {
    /// Constructs [`TopologicalIterator`] from a list of `canonical_txs` (e.g [`CanonicalIter`]),
    /// in order to handle all the graph building internally.
    pub(crate) fn new(
        canonical_txs: impl Iterator<Item = CanonicalTx<'a, Arc<Transaction>, A>>,
    ) -> Self {
        // Build a map from txid to canonical tx for quick lookup
        let mut tx_map: HashMap<Txid, CanonicalTx<'a, Arc<Transaction>, A>> = HashMap::new();
        let mut canonical_set: HashSet<Txid> = HashSet::new();

        for canonical_tx in canonical_txs {
            let txid = canonical_tx.tx_node.txid;
            canonical_set.insert(txid);
            tx_map.insert(txid, canonical_tx);
        }

        // Build the dependency graph (txid -> parents it depends on)
        let mut dependencies: HashMap<Txid, Vec<Txid>> = HashMap::new();
        let mut has_parents: HashSet<Txid> = HashSet::new();

        for &txid in canonical_set.iter() {
            let canonical_tx = tx_map.get(&txid).expect("txid must exist in map");
            let tx = &canonical_tx.tx_node.tx;

            // Find all parents (transactions this one depends on)
            let mut parents = Vec::new();
            if !tx.is_coinbase() {
                for txin in &tx.input {
                    let parent_txid = txin.previous_output.txid;
                    // Only include if the parent is also canonical
                    if canonical_set.contains(&parent_txid) {
                        parents.push(parent_txid);
                        has_parents.insert(txid);
                    }
                }
            }

            if !parents.is_empty() {
                dependencies.insert(txid, parents);
            }
        }

        // Build adjacency list and parent counts for traversal
        let mut parent_count = HashMap::new();
        let mut children_map: HashMap<Txid, Vec<Txid>> = HashMap::new();

        for (txid, parents) in &dependencies {
            for parent_txid in parents {
                children_map.entry(*parent_txid).or_default().push(*txid);
                *parent_count.entry(*txid).or_insert(0) += 1;
            }
        }

        // Find root transactions (those with no parents in the canonical set)
        let roots: Vec<Txid> = canonical_set
            .iter()
            .filter(|&&txid| !has_parents.contains(&txid))
            .copied()
            .collect();

        // Sort the initial level
        let mut current_level = roots;
        Self::sort_level_by_chain_position(&mut current_level, &tx_map);

        Self {
            canonical_txs: tx_map,
            current_level,
            next_level: Vec::new(),
            children_map,
            parent_count,
            current_index: 0,
        }
    }

    /// Sort transactions within a level by their chain position
    /// Confirmed transactions come first (sorted by height), then unconfirmed (sorted by last_seen)
    fn sort_level_by_chain_position(
        level: &mut [Txid],
        canonical_txs: &HashMap<Txid, CanonicalTx<'a, Arc<Transaction>, A>>,
    ) {
        level.sort_by(|&a_txid, &b_txid| {
            let a_tx = canonical_txs.get(&a_txid).expect("txid must exist");
            let b_tx = canonical_txs.get(&b_txid).expect("txid must exist");

            a_tx.cmp(b_tx)
        });
    }

    fn advance_to_next_level(&mut self) {
        self.current_level = core::mem::take(&mut self.next_level);
        Self::sort_level_by_chain_position(&mut self.current_level, &self.canonical_txs);
        self.current_index = 0;
    }
}

impl<'a, A: Clone + Anchor> Iterator for TopologicalIterator<'a, A> {
    type Item = CanonicalTx<'a, Arc<Transaction>, A>;

    fn next(&mut self) -> Option<Self::Item> {
        // If we've exhausted the current level, move to next
        if self.current_index >= self.current_level.len() {
            if self.next_level.is_empty() {
                return None;
            }
            self.advance_to_next_level();
        }

        let current_txid = self.current_level[self.current_index];
        self.current_index += 1;

        // If this is the last item in current level, prepare dependents for next level
        if self.current_index == self.current_level.len() {
            // Process all dependents of all transactions in current level
            for &tx in &self.current_level {
                if let Some(children) = self.children_map.get(&tx) {
                    for &child in children {
                        if let Some(count) = self.parent_count.get_mut(&child) {
                            *count -= 1;
                            if *count == 0 {
                                self.next_level.push(child);
                            }
                        }
                    }
                }
            }
        }

        // Return the CanonicalTx for the current txid
        self.canonical_txs.get(&current_txid).cloned()
    }
}
