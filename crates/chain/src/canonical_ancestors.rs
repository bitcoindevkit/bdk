//! Reverse-topological traversal of canonical ancestors.
//!
//! This module provides [`CanonicalAncestors`], an iterator over the canonical ancestors of a set
//! of seed transactions, accumulating a [`Merge`]able value from each transaction's descendants.

use crate::collections::{HashMap, HashSet, VecDeque};
use alloc::vec::Vec;

use bitcoin::Txid;

use crate::{Canonical, CanonicalTx, Merge};

impl<A, P> Canonical<A, P> {
    /// Walk the canonical ancestors of the given `seeds`, accumulating a [`Merge`]able value.
    ///
    /// Ancestors are yielded in reverse topological order — each transaction is visited only after
    /// every descendant within the traversed set that spends it has been visited — and each
    /// transaction is visited exactly once.
    ///
    /// Accumulation is a fold over the ancestor DAG:
    ///
    /// * `map` computes a transaction's *own* contribution from the transaction.
    /// * A transaction's final accumulator is its own contribution [merged](Merge::merge) with the
    ///   final accumulators of every descendant within the traversed set. Where multiple descendant
    ///   paths converge on the same ancestor, their accumulators are all merged in.
    ///
    /// `should_walk` controls pruning: returning `false` for a transaction stops the traversal from
    /// descending into *its* ancestors (the transaction itself is still visited). The decision is
    /// made from the transaction (and its [position](CanonicalTx::pos)) alone, so it is
    /// well-defined before any accumulation happens.
    ///
    /// The `seeds` are the txids of the starting transactions. Seeds that are not part of this
    /// canonical set are ignored. Seeds are not yielded; they only seed the traversal and propagate
    /// their accumulators into their ancestors.
    ///
    /// Each item is a `(accumulator, ancestor)` pair, where `accumulator` is the ancestor's final
    /// (fully merged) accumulator.
    ///
    /// # Note on merge order
    ///
    /// The order in which descendant accumulators are merged into a shared ancestor is unspecified.
    /// For order-independent (idempotent / commutative) [`Merge`] implementations the result is
    /// deterministic regardless; for order-sensitive implementations (e.g. last-write-wins) the
    /// outcome at a convergence point may depend on traversal order.
    pub fn ancestors<T, M, S>(
        &self,
        seeds: impl IntoIterator<Item = Txid>,
        map: M,
        should_walk: S,
    ) -> CanonicalAncestors<'_, A, P, T>
    where
        P: Clone,
        T: Merge + Clone,
        M: FnMut(CanonicalTx<P>) -> T,
        S: FnMut(CanonicalTx<P>) -> bool,
    {
        CanonicalAncestors::new(self, seeds, map, should_walk)
    }
}

/// An iterator over the canonical ancestors of a set of seed transactions.
///
/// Created by [`Canonical::ancestors`]. Ancestors are yielded in reverse topological order (each
/// transaction is visited only after every descendant within the traversed set that spends it has
/// been visited) and each transaction is visited exactly once. Each transaction's accumulator is
/// its own contribution (from `map`) [merged](Merge::merge) with the accumulators of all its
/// descendants within the traversed set.
///
/// The seed transactions themselves are *not* yielded; they only seed the accumulation.
///
/// Each item is a `(accumulator, ancestor)` pair.
pub struct CanonicalAncestors<'c, A, P, T> {
    canonical: &'c Canonical<A, P>,
    /// The set of seed txids, excluded from emission.
    seeds: HashSet<Txid>,
    /// For each visited tx, the distinct in-set parents reached *through* it (empty if pruned).
    parents: HashMap<Txid, Vec<Txid>>,
    /// Remaining number of in-set descendants for each tx; a tx is ready once this hits zero.
    in_degree: HashMap<Txid, usize>,
    /// Accumulator per tx. Seeded with the tx's own contribution, then descendants are merged in.
    acc: HashMap<Txid, T>,
    /// Txs whose `in_degree` has reached zero and are ready to be finalized/emitted.
    ready: VecDeque<Txid>,
    /// Number of non-seed transactions yet to be yielded.
    remaining: usize,
}

impl<'c, A, P, T> CanonicalAncestors<'c, A, P, T> {
    pub(crate) fn new<M, S>(
        canonical: &'c Canonical<A, P>,
        seeds: impl IntoIterator<Item = Txid>,
        mut map: M,
        mut should_walk: S,
    ) -> Self
    where
        P: Clone,
        M: FnMut(CanonicalTx<P>) -> T,
        S: FnMut(CanonicalTx<P>) -> bool,
    {
        let mut seeds_set = HashSet::<Txid>::new();
        let mut reachable = HashSet::<Txid>::new();
        let mut acc = HashMap::<Txid, T>::new();
        let mut stack = Vec::<Txid>::new();

        for txid in seeds {
            // Only transactions in this canonical set participate.
            if !canonical.txs.contains_key(&txid) {
                continue;
            }
            seeds_set.insert(txid);
            if reachable.insert(txid) {
                stack.push(txid);
            }
        }

        // Phase 1: discover the reachable subgraph and in-set edges (respecting pruning), seeding
        // each visited tx's accumulator with its own contribution.
        let mut parents = HashMap::<Txid, Vec<Txid>>::new();
        let mut in_degree = HashMap::<Txid, usize>::new();
        while let Some(txid) = stack.pop() {
            // Present because we only ever push txids found in `canonical.txs`.
            let (tx, pos) = &canonical.txs[&txid];
            let canonical_tx = || CanonicalTx {
                pos: pos.clone(),
                txid,
                tx: tx.clone(),
            };
            acc.insert(txid, map(canonical_tx()));

            if !should_walk(canonical_tx()) {
                parents.insert(txid, Vec::new());
                continue;
            }
            let mut my_parents = Vec::<Txid>::new();
            let mut seen = HashSet::<Txid>::new();
            for txin in &tx.input {
                let parent_txid = txin.previous_output.txid;
                // distinct parents only, and only those that are canonical
                if !seen.insert(parent_txid) {
                    continue;
                }
                if canonical.txs.contains_key(&parent_txid) {
                    my_parents.push(parent_txid);
                    *in_degree.entry(parent_txid).or_insert(0) += 1;
                    if reachable.insert(parent_txid) {
                        stack.push(parent_txid);
                    }
                }
            }
            parents.insert(txid, my_parents);
        }

        // Phase 2 seed: every reachable tx with no in-set descendants is ready immediately.
        let mut ready = VecDeque::<Txid>::new();
        for &txid in &reachable {
            if in_degree.get(&txid).copied().unwrap_or(0) == 0 {
                ready.push_back(txid);
            }
        }

        let remaining = reachable.len() - seeds_set.len();

        Self {
            canonical,
            seeds: seeds_set,
            parents,
            in_degree,
            acc,
            ready,
            remaining,
        }
    }
}

impl<A, P, T> Iterator for CanonicalAncestors<'_, A, P, T>
where
    P: Clone,
    T: Merge + Clone,
{
    type Item = (T, CanonicalTx<P>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let txid = self.ready.pop_front()?;

            // The accumulator for `txid` is now finalized: all descendants have been merged in.
            let node_acc = self
                .acc
                .remove(&txid)
                .expect("accumulator must exist once a tx is ready");

            // Merge this tx's accumulator into each of its in-set parents.
            let parents = self.parents.remove(&txid).unwrap_or_default();
            for parent_txid in parents {
                self.acc
                    .get_mut(&parent_txid)
                    .expect("parent accumulator was seeded during discovery")
                    .merge(node_acc.clone());

                let d = self
                    .in_degree
                    .get_mut(&parent_txid)
                    .expect("parent must have an in-degree entry");
                *d -= 1;
                if *d == 0 {
                    self.ready.push_back(parent_txid);
                }
            }

            // Seeds seed the accumulation but are not themselves yielded.
            if self.seeds.contains(&txid) {
                continue;
            }

            let (tx, pos) = self.canonical.txs[&txid].clone();
            self.remaining -= 1;
            return Some((node_acc, CanonicalTx { pos, txid, tx }));
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<A, P, T> ExactSizeIterator for CanonicalAncestors<'_, A, P, T>
where
    P: Clone,
    T: Merge + Clone,
{
    fn len(&self) -> usize {
        self.remaining
    }
}
