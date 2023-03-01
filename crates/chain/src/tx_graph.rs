//! Module for structures that store and traverse transactions.
//!
//! [`TxGraph`] is a monotone structure that inserts transactions and indexes spends. The
//! [`Additions`] structure reports changes of [`TxGraph`], but can also be applied on to a
//! [`TxGraph`] as well. Lastly, [`TxDescendants`] is an [`Iterator`] which traverses descendants of
//! a given transaction.
//!
//! Conflicting transactions are allowed to coexist within a [`TxGraph`]. This is useful for
//! identifying and traversing conflicts and descendants of a given transaction.
//!
//! # Previewing and applying changes
//!
//! Methods that either preview or apply changes to [`TxGraph`] will return [`Additions`].
//! [`Additions`] can be applied back on to a [`TxGraph`], or be used to inform persistent storage
//! of the changes to [`TxGraph`].
//!
//! ```
//! # use bdk_chain::tx_graph::TxGraph;
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! # let tx_b = tx_from_hex(RAW_TX_2);
//! let mut graph = TxGraph::<Transaction>::default();
//!
//! // preview a transaction insertion (not actually inserted)
//! let additions = graph.insert_tx_preview(tx_a);
//! // apply the insertion
//! graph.apply_additions(additions);
//!
//! // you can also insert a transaction directly
//! let already_applied_additions = graph.insert_tx(tx_b);
//! ```
//!
//! A [`TxGraph`] can also be updated with another [`TxGraph`].
//!
//! ```
//! # use bdk_chain::tx_graph::TxGraph;
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! # let tx_b = tx_from_hex(RAW_TX_2);
//! let mut graph = TxGraph::<Transaction>::default();
//! let update = TxGraph::<Transaction>::new(vec![tx_a, tx_b]);
//!
//! // preview additions as result of the update
//! let additions = graph.determine_additions(&update);
//! // apply the additions
//! graph.apply_additions(additions);
//!
//! // we can also apply the update graph directly
//! // the additions will be empty as we have already applied the same update above
//! let additions = graph.apply_update(update);
//! assert!(additions.is_empty());
//! ```
//!
use crate::{collections::*, AsTransaction, ForEachTxOut, IntoOwned};
use alloc::vec::Vec;
use bitcoin::{OutPoint, Transaction, TxOut, Txid};
use core::ops::RangeInclusive;

/// A graph of transactions and spends.
///
/// See the [module-level documentation] for more.
///
/// [module-level documentation]: crate::tx_graph
#[derive(Clone, Debug, PartialEq)]
pub struct TxGraph<T = Transaction> {
    txs: HashMap<Txid, TxNode<T>>,
    spends: BTreeMap<OutPoint, HashSet<Txid>>,

    // This atrocity exists so that `TxGraph::outspends()` can return a reference.
    // FIXME: This can be removed once `HashSet::new` is a const fn.
    empty_outspends: HashSet<Txid>,
}

impl<T> Default for TxGraph<T> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            spends: Default::default(),
            empty_outspends: Default::default(),
        }
    }
}

/// Node of a [`TxGraph`]. This can either be a whole transaction, or a partial transaction (where
/// we only have select outputs).
#[derive(Clone, Debug, PartialEq)]
enum TxNode<T = Transaction> {
    Whole(T),
    Partial(BTreeMap<u32, TxOut>),
}

impl<T> Default for TxNode<T> {
    fn default() -> Self {
        Self::Partial(BTreeMap::new())
    }
}

impl<T: AsTransaction> TxGraph<T> {
    /// Iterate over all tx outputs known by [`TxGraph`].
    pub fn all_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs.iter().flat_map(|(txid, tx)| match tx {
            TxNode::Whole(tx) => tx
                .as_tx()
                .output
                .iter()
                .enumerate()
                .map(|(vout, txout)| (OutPoint::new(*txid, vout as _), txout))
                .collect::<Vec<_>>(),
            TxNode::Partial(txouts) => txouts
                .iter()
                .map(|(vout, txout)| (OutPoint::new(*txid, *vout as _), txout))
                .collect::<Vec<_>>(),
        })
    }

    /// Iterate over all full transactions in the graph.
    pub fn full_transactions(&self) -> impl Iterator<Item = &T> {
        self.txs.iter().filter_map(|(_, tx)| match tx {
            TxNode::Whole(tx) => Some(tx),
            TxNode::Partial(_) => None,
        })
    }

    /// Get a transaction by txid. This only returns `Some` for full transactions.
    ///
    /// Refer to [`get_txout`] for getting a specific [`TxOut`].
    ///
    /// [`get_txout`]: Self::get_txout
    pub fn get_tx(&self, txid: Txid) -> Option<&T> {
        match self.txs.get(&txid)? {
            TxNode::Whole(tx) => Some(tx),
            TxNode::Partial(_) => None,
        }
    }

    /// Obtains a single tx output (if any) at specified outpoint.
    pub fn get_txout(&self, outpoint: OutPoint) -> Option<&TxOut> {
        match self.txs.get(&outpoint.txid)? {
            TxNode::Whole(tx) => tx.as_tx().output.get(outpoint.vout as usize),
            TxNode::Partial(txouts) => txouts.get(&outpoint.vout),
        }
    }

    /// Returns a [`BTreeMap`] of vout to output of the provided `txid`.
    pub fn txouts(&self, txid: Txid) -> Option<BTreeMap<u32, &TxOut>> {
        Some(match self.txs.get(&txid)? {
            TxNode::Whole(tx) => tx
                .as_tx()
                .output
                .iter()
                .enumerate()
                .map(|(vout, txout)| (vout as u32, txout))
                .collect::<BTreeMap<_, _>>(),
            TxNode::Partial(txouts) => txouts
                .iter()
                .map(|(vout, txout)| (*vout, txout))
                .collect::<BTreeMap<_, _>>(),
        })
    }

    /// Calculates the fee of a given transaction. Returns 0 if `tx` is a coinbase transaction.
    /// Returns `Some(_)` if we have all the `TxOut`s being spent by `tx` in the graph (either as
    /// the full transactions or individual txouts). If the returned value is negative then the
    /// transaction is invalid according to the graph.
    ///
    /// Returns `None` if we're missing an input for the tx in the graph.
    ///
    /// Note `tx` does not have to be in the graph for this to work.
    pub fn calculate_fee(&self, tx: &Transaction) -> Option<i64> {
        if tx.is_coin_base() {
            return Some(0);
        }
        let inputs_sum = tx
            .input
            .iter()
            .map(|txin| {
                self.get_txout(txin.previous_output)
                    .map(|txout| txout.value as i64)
            })
            .sum::<Option<i64>>()?;

        let outputs_sum = tx
            .output
            .iter()
            .map(|txout| txout.value as i64)
            .sum::<i64>();

        Some(inputs_sum - outputs_sum)
    }
}

impl<T: AsTransaction + Ord + Clone> TxGraph<T> {
    /// Contruct a new [`TxGraph`] from a list of transaction.
    pub fn new(txs: impl IntoIterator<Item = T>) -> Self {
        let mut new = Self::default();
        for tx in txs.into_iter() {
            let _ = new.insert_tx(tx);
        }
        new
    }
    /// Inserts the given [`TxOut`] at [`OutPoint`].
    ///
    /// Note this will ignore the action if we already have the full transaction that the txout is
    /// alledged to be on (even if it doesn't match it!).
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) -> Additions<T> {
        let additions = self.insert_txout_preview(outpoint, txout);
        self.apply_additions(additions.clone());
        additions
    }

    /// Inserts the given transaction into [`TxGraph`].
    ///
    /// The [`Additions`] returned will be empty if `tx` already exists.
    pub fn insert_tx(&mut self, tx: T) -> Additions<T> {
        let additions = self.insert_tx_preview(tx);
        self.apply_additions(additions.clone());
        additions
    }

    /// Extends this graph with another so that `self` becomes the union of the two sets of
    /// transactions.
    ///
    /// The returned [`Additions`] is the set difference of `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub fn apply_update<T2>(&mut self, update: TxGraph<T2>) -> Additions<T>
    where
        T2: IntoOwned<T> + Clone,
    {
        let additions = self.determine_additions(&update);
        self.apply_additions(additions.clone());
        additions
    }

    /// Applies [`Additions`] to [`TxGraph`].
    pub fn apply_additions(&mut self, additions: Additions<T>) {
        for tx in additions.tx {
            let txid = tx.as_tx().txid();

            tx.as_tx()
                .input
                .iter()
                .map(|txin| txin.previous_output)
                // coinbase spends are not to be counted
                .filter(|outpoint| !outpoint.is_null())
                // record spend as this tx has spent this outpoint
                .for_each(|outpoint| {
                    self.spends.entry(outpoint).or_default().insert(txid);
                });

            if let Some(TxNode::Whole(old_tx)) = self.txs.insert(txid, TxNode::Whole(tx)) {
                debug_assert_eq!(
                    old_tx.as_tx().txid(),
                    txid,
                    "old tx of same txid should not be different"
                );
            }
        }

        for (outpoint, txout) in additions.txout {
            let tx_entry = self
                .txs
                .entry(outpoint.txid)
                .or_insert_with(TxNode::default);

            match tx_entry {
                TxNode::Whole(_) => { /* do nothing since we already have full tx */ }
                TxNode::Partial(txouts) => {
                    txouts.insert(outpoint.vout, txout);
                }
            }
        }
    }

    /// Previews the resultant [`Additions`] when [`Self`] is updated against the `update` graph.
    ///
    /// The [`Additions`] would be the set difference of `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub fn determine_additions<'a, T2>(&self, update: &'a TxGraph<T2>) -> Additions<T>
    where
        T2: IntoOwned<T> + Clone,
    {
        let mut additions = Additions::<T>::default();

        for (&txid, update_tx) in &update.txs {
            if self.get_tx(txid).is_some() {
                continue;
            }

            match update_tx {
                TxNode::Whole(tx) => {
                    if matches!(self.txs.get(&txid), None | Some(TxNode::Partial(_))) {
                        additions
                            .tx
                            .insert(<T2 as IntoOwned<T>>::into_owned(tx.clone()));
                    }
                }
                TxNode::Partial(partial) => {
                    for (&vout, update_txout) in partial {
                        let outpoint = OutPoint::new(txid, vout);

                        if self.get_txout(outpoint) != Some(&update_txout) {
                            additions.txout.insert(outpoint, update_txout.clone());
                        }
                    }
                }
            }
        }

        additions
    }

    /// Returns the resultant [`Additions`] if the given transaction is inserted. Does not actually
    /// mutate [`Self`].
    ///
    /// The [`Additions`] result will be empty if `tx` already existed in `self`.
    pub fn insert_tx_preview(&self, tx: T) -> Additions<T> {
        let mut update = Self::default();
        update.txs.insert(tx.as_tx().txid(), TxNode::Whole(tx));
        self.determine_additions(&update)
    }

    /// Returns the resultant [`Additions`] if the given `txout` is inserted at `outpoint`. Does not
    /// mutate `self`.
    ///
    /// The [`Additions`] result will be empty if the `outpoint` (or a full transaction containing
    /// the `outpoint`) already existed in `self`.
    pub fn insert_txout_preview(&self, outpoint: OutPoint, txout: TxOut) -> Additions<T> {
        let mut update = Self::default();
        update.txs.insert(
            outpoint.txid,
            TxNode::Partial([(outpoint.vout, txout)].into()),
        );
        self.determine_additions(&update)
    }
}

impl<T> TxGraph<T> {
    /// The transactions spending from this output.
    ///
    /// `TxGraph` allows conflicting transactions within the graph. Obviously the transactions in
    /// the returned will never be in the same blockchain.
    pub fn outspends(&self, outpoint: OutPoint) -> &HashSet<Txid> {
        self.spends.get(&outpoint).unwrap_or(&self.empty_outspends)
    }

    /// Iterates over the transactions spending from `txid`.
    ///
    /// The iterator item is a union of `(vout, txid-set)` where:
    ///
    /// - `vout` is the provided `txid`'s outpoint that is being spent
    /// - `txid-set` is the set of txids that is spending the `vout`
    pub fn tx_outspends(
        &self,
        txid: Txid,
    ) -> impl DoubleEndedIterator<Item = (u32, &HashSet<Txid>)> + '_ {
        let start = OutPoint { txid, vout: 0 };
        let end = OutPoint {
            txid,
            vout: u32::MAX,
        };
        self.spends
            .range(start..=end)
            .map(|(outpoint, spends)| (outpoint.vout, spends))
    }

    /// Iterate over all partial transactions (outputs only) in the graph.
    pub fn partial_transactions(&self) -> impl Iterator<Item = (Txid, &BTreeMap<u32, TxOut>)> {
        self.txs.iter().filter_map(|(txid, tx)| match tx {
            TxNode::Whole(_) => None,
            TxNode::Partial(partial) => Some((*txid, partial)),
        })
    }

    /// Creates an iterator that both filters and maps descendants from the starting `txid`.
    ///
    /// The supplied closure takes in two inputs `(depth, descendant_txid)`:
    ///
    /// * `depth` is the distance between the starting `txid` and the `descendant_txid`. I.e. if the
    ///     descendant is spending an output of the starting `txid`, the `depth` will be 1.
    /// * `descendant_txid` is the descendant's txid which we are considering to walk.
    ///
    /// The supplied closure returns an `Option<T>`, allowing the caller to map each node it vists
    /// and decide whether to visit descendants.
    pub fn walk_descendants<'g, F, O>(&'g self, txid: Txid, walk_map: F) -> TxDescendants<F, T>
    where
        F: FnMut(usize, Txid) -> Option<O> + 'g,
    {
        TxDescendants::new_exclude_root(self, txid, walk_map)
    }

    /// Creates an iterator that both filters and maps conflicting transactions (this includes
    /// descendants of directly-conflicting transactions, which are also considered conflicts).
    ///
    /// Refer to [`Self::walk_descendants`] for `walk_map` usage.
    pub fn walk_conflicts<'g, F, O>(
        &'g self,
        tx: &'g Transaction,
        walk_map: F,
    ) -> TxDescendants<F, T>
    where
        F: FnMut(usize, Txid) -> Option<O> + 'g,
    {
        let txids = self.direct_conflicts_of_tx(tx).map(|(_, txid)| txid);
        TxDescendants::from_multiple_include_root(self, txids, walk_map)
    }

    /// Given a transaction, return an iterator of txids which directly conflict with the given
    /// transaction's inputs (spends). The conflicting txids are returned with the given
    /// transaction's vin (in which it conflicts).
    ///
    /// Note that this only returns directly conflicting txids and does not include descendants of
    /// those txids (which are technically also conflicting).
    pub fn direct_conflicts_of_tx<'g>(
        &'g self,
        tx: &'g Transaction,
    ) -> impl Iterator<Item = (usize, Txid)> + '_ {
        let txid = tx.txid();
        tx.input
            .iter()
            .enumerate()
            .filter_map(|(vin, txin)| self.spends.get(&txin.previous_output).zip(Some(vin)))
            .flat_map(|(spends, vin)| core::iter::repeat(vin).zip(spends.iter().cloned()))
            .filter(move |(_, conflicting_txid)| *conflicting_txid != txid)
    }

    /// Whether the graph has any transactions or outputs in it.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }
}

/// A structure that represents changes to a [`TxGraph`].
///
/// It is named "additions" because [`TxGraph`] is monotone so transactions can only be added and
/// not removed.
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate::tx_graph
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(
        crate = "serde_crate",
        bound(
            deserialize = "T: Ord + serde::Deserialize<'de>",
            serialize = "T: Ord + serde::Serialize"
        )
    )
)]
#[must_use]
pub struct Additions<T> {
    pub tx: BTreeSet<T>,
    pub txout: BTreeMap<OutPoint, TxOut>,
}

impl<T> Additions<T> {
    /// Returns true if the [`Additions`] is empty (no transactions or txouts).
    pub fn is_empty(&self) -> bool {
        self.tx.is_empty() && self.txout.is_empty()
    }

    /// Iterates over all outpoints contained within [`Additions`].
    pub fn txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)>
    where
        T: AsTransaction,
    {
        self.tx
            .iter()
            .flat_map(|tx| {
                tx.as_tx()
                    .output
                    .iter()
                    .enumerate()
                    .map(|(vout, txout)| (OutPoint::new(tx.as_tx().txid(), vout as _), txout))
            })
            .chain(self.txout.iter().map(|(op, txout)| (*op, txout)))
    }

    /// Appends the changes in `other` into self such that applying `self` afterwards has the same
    /// effect as sequentially applying the original `self` and `other`.
    pub fn append(&mut self, mut other: Additions<T>)
    where
        T: Ord,
    {
        self.tx.append(&mut other.tx);
        self.txout.append(&mut other.txout);
    }
}

impl<T> Default for Additions<T> {
    fn default() -> Self {
        Self {
            tx: Default::default(),
            txout: Default::default(),
        }
    }
}

impl AsRef<TxGraph> for TxGraph {
    fn as_ref(&self) -> &TxGraph {
        self
    }
}

impl<T: AsTransaction> ForEachTxOut for Additions<T> {
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut))) {
        self.txouts().for_each(f)
    }
}

impl<T: AsTransaction> ForEachTxOut for TxGraph<T> {
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut))) {
        self.all_txouts().for_each(f)
    }
}

/// An iterator that traverses transaction descendants.
///
/// This `struct` is created by the [`walk_descendants`] method of [`TxGraph`].
///
/// [`walk_descendants`]: TxGraph::walk_descendants
pub struct TxDescendants<'g, F, T> {
    graph: &'g TxGraph<T>,
    visited: HashSet<Txid>,
    stack: Vec<(usize, Txid)>,
    filter_map: F,
}

impl<'g, F, T> TxDescendants<'g, F, T> {
    /// Creates a `TxDescendants` that includes the starting `txid` when iterating.
    #[allow(unused)]
    pub(crate) fn new_include_root(graph: &'g TxGraph<T>, txid: Txid, filter_map: F) -> Self {
        Self {
            graph,
            visited: Default::default(),
            stack: [(0, txid)].into(),
            filter_map,
        }
    }

    /// Creates a `TxDescendants` that excludes the starting `txid` when iterating.
    pub(crate) fn new_exclude_root(graph: &'g TxGraph<T>, txid: Txid, filter_map: F) -> Self {
        let mut descendants = Self {
            graph,
            visited: Default::default(),
            stack: Default::default(),
            filter_map,
        };
        descendants.populate_stack(1, txid);
        descendants
    }

    /// Creates a `TxDescendants` from multiple starting transactions that includes the starting
    /// `txid`s when iterating.
    pub(crate) fn from_multiple_include_root<I>(
        graph: &'g TxGraph<T>,
        txids: I,
        filter_map: F,
    ) -> Self
    where
        I: IntoIterator<Item = Txid>,
    {
        Self {
            graph,
            visited: Default::default(),
            stack: txids.into_iter().map(|txid| (0, txid)).collect(),
            filter_map,
        }
    }

    /// Creates a `TxDescendants` from multiple starting transactions that excludes the starting
    /// `txid`s when iterating.
    #[allow(unused)]
    pub(crate) fn from_multiple_exclude_root<I>(
        graph: &'g TxGraph<T>,
        txids: I,
        filter_map: F,
    ) -> Self
    where
        I: IntoIterator<Item = Txid>,
    {
        let mut descendants = Self {
            graph,
            visited: Default::default(),
            stack: Default::default(),
            filter_map,
        };
        for txid in txids {
            descendants.populate_stack(1, txid);
        }
        descendants
    }
}

impl<'g, F, T> TxDescendants<'g, F, T> {
    fn populate_stack(&mut self, depth: usize, txid: Txid) {
        let spend_paths = self
            .graph
            .spends
            .range(tx_outpoint_range(txid))
            .flat_map(|(_, spends)| spends)
            .map(|&txid| (depth, txid));
        self.stack.extend(spend_paths);
    }
}

impl<'g, F, O, T> Iterator for TxDescendants<'g, F, T>
where
    F: FnMut(usize, Txid) -> Option<O>,
{
    type Item = O;

    fn next(&mut self) -> Option<Self::Item> {
        let (op_spends, txid, item) = loop {
            // we have exhausted all paths when stack is empty
            let (op_spends, txid) = self.stack.pop()?;
            // we do not want to visit the same transaction twice
            if self.visited.insert(txid) {
                // ignore paths when user filters them out
                if let Some(item) = (self.filter_map)(op_spends, txid) {
                    break (op_spends, txid, item);
                }
            }
        };

        self.populate_stack(op_spends + 1, txid);
        return Some(item);
    }
}

fn tx_outpoint_range(txid: Txid) -> RangeInclusive<OutPoint> {
    OutPoint::new(txid, u32::MIN)..=OutPoint::new(txid, u32::MAX)
}
