use crate::collections::{hash_map, HashMap, HashSet, VecDeque};
use crate::tx_graph::{TxAncestors, TxDescendants};
use crate::{Anchor, ChainOracle, TxGraph};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::BlockId;
use bitcoin::{Transaction, Txid};

/// Modifies the canonicalization algorithm.
#[non_exhaustive]
#[derive(Debug, Default, Clone)]
pub struct CanonicalizationMods {
    /// Transactions that will supercede all other transactions.
    ///
    /// In case of conflicting transactions within `assume_canonical`, transactions that appear
    /// later in the list (have higher index) have precedence.
    ///
    /// If the same transaction exists in both `assume_canonical` and `assume_not_canonical`,
    /// `assume_not_canonical` will take precedence.
    pub assume_canonical: Vec<Txid>,

    /// Transactions that will never be considered canonical.
    ///
    /// Descendants of these transactions will also be evicted.
    ///
    /// If the same transaction exists in both `assume_canonical` and `assume_not_canonical`,
    /// `assume_not_canonical` will take precedence.
    pub assume_not_canonical: Vec<Txid>,
}

impl CanonicalizationMods {
    /// No mods.
    pub const NONE: Self = Self {
        assume_canonical: Vec::new(),
        assume_not_canonical: Vec::new(),
    };
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

    canonical: HashMap<Txid, (Arc<Transaction>, CanonicalReason<A>)>,
    not_canonical: HashSet<Txid>,

    queue: VecDeque<Txid>,
}

impl<'g, A: Anchor, C: ChainOracle> CanonicalIter<'g, A, C> {
    /// Constructs [`CanonicalIter`].
    pub fn new(
        tx_graph: &'g TxGraph<A>,
        chain: &'g C,
        chain_tip: BlockId,
        mods: CanonicalizationMods,
    ) -> Self {
        let mut not_canonical = HashSet::new();
        for txid in mods.assume_not_canonical {
            Self::_mark_not_canonical(tx_graph, &mut not_canonical, txid);
        }
        let anchors = tx_graph.all_anchors();
        let unprocessed_assumed_txs = Box::new(
            mods.assume_canonical
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
            not_canonical,
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
                    "tx taken from `unprocessed_txs_with_anchors` so it must atleast have an anchor",
                )
                .confirmation_height_upper_bound(),
        ));
        Ok(())
    }

    /// Marks a transaction and it's ancestors as canonical. Mark all conflicts of these as
    /// `not_canonical`.
    fn mark_canonical(&mut self, txid: Txid, tx: Arc<Transaction>, reason: CanonicalReason<A>) {
        let starting_txid = txid;
        let mut is_root = true;
        TxAncestors::new_include_root(
            self.tx_graph,
            tx,
            |_: usize, tx: Arc<Transaction>| -> Option<()> {
                let this_txid = tx.compute_txid();
                let this_reason = if is_root {
                    is_root = false;
                    reason.clone()
                } else {
                    reason.to_transitive(starting_txid)
                };
                let canonical_entry = match self.canonical.entry(this_txid) {
                    // Already visited tx before, exit early.
                    hash_map::Entry::Occupied(_) => return None,
                    hash_map::Entry::Vacant(entry) => entry,
                };
                // Any conflicts with a canonical tx can be added to `not_canonical`. Descendants
                // of `not_canonical` txs can also be added to `not_canonical`.
                for (_, conflict_txid) in self.tx_graph.direct_conflicts(&tx) {
                    Self::_mark_not_canonical(
                        self.tx_graph,
                        &mut self.not_canonical,
                        conflict_txid,
                    );
                }
                canonical_entry.insert((tx, this_reason));
                self.queue.push_back(this_txid);
                Some(())
            },
        )
        .run_until_finished()
    }

    fn _mark_not_canonical(tx_graph: &TxGraph<A>, not_canonical: &mut HashSet<Txid>, txid: Txid) {
        TxDescendants::new_include_root(tx_graph, txid, |_: usize, txid: Txid| -> Option<()> {
            if not_canonical.insert(txid) {
                Some(())
            } else {
                None
            }
        })
        .run_until_finished();
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
                if !self.is_canonicalized(txid) {
                    let observed_in = ObservedIn::Mempool(last_seen);
                    self.mark_canonical(txid, tx, CanonicalReason::from_observed_in(observed_in));
                }
                continue;
            }

            if let Some((txid, tx, height)) = self.unprocessed_leftover_txs.pop_front() {
                if !self.is_canonicalized(txid) {
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
