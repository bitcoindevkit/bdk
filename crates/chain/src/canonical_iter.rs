use crate::collections::{HashMap, HashSet, VecDeque};
use crate::tx_graph::{TxAncestors, TxDescendants};
use crate::{Anchor, TxGraph};
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
    /// Transactions that will supercede all other transactions.
    ///
    /// In case of conflicting transactions within `assume_canonical`, transactions that appear
    /// later in the list (have higher index) have precedence.
    pub assume_canonical: Vec<Txid>,
}

/// Iterates over canonical txs.
pub struct CanonicalIter<'g, A, F>
where
    F: FnMut(BlockId) -> Option<bool>,
{
    tx_graph: &'g TxGraph<A>,
    is_block_in_chain: F,

    unprocessed_assumed_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>)> + 'g>,
    unprocessed_anchored_txs:
        Box<dyn Iterator<Item = (Txid, Arc<Transaction>, &'g BTreeSet<A>)> + 'g>,
    unprocessed_seen_txs: Box<dyn Iterator<Item = (Txid, Arc<Transaction>, u64)> + 'g>,
    unprocessed_leftover_txs: VecDeque<(Txid, Arc<Transaction>, u32)>,

    canonical: CanonicalMap<A>,
    not_canonical: NotCanonicalSet,

    queue: VecDeque<Txid>,
}

impl<'g, A: Anchor, F> CanonicalIter<'g, A, F>
where
    F: FnMut(BlockId) -> Option<bool>,
{
    /// Constructs [`CanonicalIter`].
    pub fn new(
        tx_graph: &'g TxGraph<A>,
        params: CanonicalizationParams,
        is_block_in_chain: F,
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
            is_block_in_chain,
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
    fn scan_anchors(&mut self, txid: Txid, tx: Arc<Transaction>, anchors: &BTreeSet<A>) {
        for anchor in anchors {
            let block_id = anchor.anchor_block();
            let is_block_in_chain = (self.is_block_in_chain)(block_id);
            if is_block_in_chain == Some(true) {
                self.mark_canonical(txid, tx, CanonicalReason::from_anchor(anchor.clone()));
                return;
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
                    "tx taken from `unprocessed_txs_with_anchors` so it must atleast have an anchor",
                )
                .confirmation_height_upper_bound(),
        ));
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

impl<A: Anchor, F> Iterator for CanonicalIter<'_, A, F>
where
    F: FnMut(BlockId) -> Option<bool>,
{
    type Item = (Txid, Arc<Transaction>, CanonicalReason<A>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(txid) = self.queue.pop_front() {
                let (tx, reason) = self
                    .canonical
                    .get(&txid)
                    .cloned()
                    .expect("reason must exist");
                return Some((txid, tx, reason));
            }

            if let Some((txid, tx)) = self.unprocessed_assumed_txs.next() {
                if !self.is_canonicalized(txid) {
                    self.mark_canonical(txid, tx, CanonicalReason::assumed());
                }
            }

            if let Some((txid, tx, anchors)) = self.unprocessed_anchored_txs.next() {
                if !self.is_canonicalized(txid) {
                    self.scan_anchors(txid, tx, anchors);
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
