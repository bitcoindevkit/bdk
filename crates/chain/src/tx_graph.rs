//! Module for structures that store and traverse transactions.
//!
//! [`TxGraph`] is a monotone structure that inserts transactions and indexes the spends. The
//! [`Additions`] structure reports changes of [`TxGraph`] but can also be applied to a
//! [`TxGraph`] as well. Lastly, [`TxDescendants`] is an [`Iterator`] that traverses descendants of
//! a given transaction.
//!
//! Conflicting transactions are allowed to coexist within a [`TxGraph`]. This is useful for
//! identifying and traversing conflicts and descendants of a given transaction.
//!
//! # Previewing and applying changes
//!
//! Methods that either preview or apply changes to [`TxGraph`] will return [`Additions`].
//! [`Additions`] can be applied back to a [`TxGraph`] or be used to inform persistent storage
//! of the changes to [`TxGraph`].
//!
//! ```
//! # use bdk_chain::BlockId;
//! # use bdk_chain::tx_graph::TxGraph;
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! # let tx_b = tx_from_hex(RAW_TX_2);
//! let mut graph = TxGraph::<BlockId>::default();
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
//! # use bdk_chain::BlockId;
//! # use bdk_chain::tx_graph::TxGraph;
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! # let tx_b = tx_from_hex(RAW_TX_2);
//! let mut graph = TxGraph::<BlockId>::default();
//! let update = TxGraph::new(vec![tx_a, tx_b]);
//!
//! // preview additions as the result of the update
//! let additions = graph.determine_additions(&update);
//! // apply the additions
//! graph.apply_additions(additions);
//!
//! // we can also apply the update graph directly
//! // the additions will be empty as we have already applied the same update above
//! let additions = graph.apply_update(update);
//! assert!(additions.is_empty());
//! ```

use crate::{collections::*, BlockAnchor, ChainOracle, ForEachTxOut, ObservedIn};
use alloc::vec::Vec;
use bitcoin::{OutPoint, Transaction, TxOut, Txid};
use core::{
    convert::Infallible,
    ops::{Deref, RangeInclusive},
};

/// A graph of transactions and spends.
///
/// See the [module-level documentation] for more.
///
/// [module-level documentation]: crate::tx_graph
#[derive(Clone, Debug, PartialEq)]
pub struct TxGraph<A = ()> {
    // all transactions that the graph is aware of in format: `(tx_node, tx_anchors, tx_last_seen)`
    txs: HashMap<Txid, (TxNode, BTreeSet<A>, u64)>,
    spends: BTreeMap<OutPoint, HashSet<Txid>>,
    anchors: BTreeSet<(A, Txid)>,

    // This atrocity exists so that `TxGraph::outspends()` can return a reference.
    // FIXME: This can be removed once `HashSet::new` is a const fn.
    empty_outspends: HashSet<Txid>,
}

impl<A> Default for TxGraph<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            spends: Default::default(),
            anchors: Default::default(),
            empty_outspends: Default::default(),
        }
    }
}

// pub type InChainTx<'a, T, A> = (ObservedIn<&'a A>, TxInGraph<'a, T, A>);
// pub type InChainTxOut<'a, I, A> = (&'a I, FullTxOut<ObservedIn<&'a A>>);

/// An outward-facing view of a transaction that resides in a [`TxGraph`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TxInGraph<'a, T, A> {
    /// Txid of the transaction.
    pub txid: Txid,
    /// A partial or full representation of the transaction.
    pub tx: &'a T,
    /// The blocks that the transaction is "anchored" in.
    pub anchors: &'a BTreeSet<A>,
    /// The last-seen unix timestamp of the transaction.
    pub last_seen: u64,
}

impl<'a, T, A> Deref for TxInGraph<'a, T, A> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.tx
    }
}

impl<'a, A> TxInGraph<'a, Transaction, A> {
    pub fn from_tx(tx: &'a Transaction, anchors: &'a BTreeSet<A>) -> Self {
        Self {
            txid: tx.txid(),
            tx,
            anchors,
            last_seen: 0,
        }
    }
}

/// Internal representation of a transaction node of a [`TxGraph`].
///
/// This can either be a whole transaction, or a partial transaction (where we only have select
/// outputs).
#[derive(Clone, Debug, PartialEq)]
enum TxNode {
    Whole(Transaction),
    Partial(BTreeMap<u32, TxOut>),
}

impl Default for TxNode {
    fn default() -> Self {
        Self::Partial(BTreeMap::new())
    }
}

impl<A> TxGraph<A> {
    /// Iterate over all tx outputs known by [`TxGraph`].
    pub fn all_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs.iter().flat_map(|(txid, (tx, _, _))| match tx {
            TxNode::Whole(tx) => tx
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
    pub fn full_transactions(&self) -> impl Iterator<Item = TxInGraph<'_, Transaction, A>> {
        self.txs
            .iter()
            .filter_map(|(&txid, (tx, anchors, last_seen))| match tx {
                TxNode::Whole(tx) => Some(TxInGraph {
                    txid,
                    tx,
                    anchors,
                    last_seen: *last_seen,
                }),
                TxNode::Partial(_) => None,
            })
    }

    /// Get a transaction by txid. This only returns `Some` for full transactions.
    ///
    /// Refer to [`get_txout`] for getting a specific [`TxOut`].
    ///
    /// [`get_txout`]: Self::get_txout
    pub fn get_tx(&self, txid: Txid) -> Option<TxInGraph<'_, Transaction, A>> {
        match &self.txs.get(&txid)? {
            (TxNode::Whole(tx), anchors, last_seen) => Some(TxInGraph {
                txid,
                tx,
                anchors,
                last_seen: *last_seen,
            }),
            _ => None,
        }
    }

    /// Obtains a single tx output (if any) at the specified outpoint.
    pub fn get_txout(&self, outpoint: OutPoint) -> Option<&TxOut> {
        match &self.txs.get(&outpoint.txid)?.0 {
            TxNode::Whole(tx) => tx.output.get(outpoint.vout as usize),
            TxNode::Partial(txouts) => txouts.get(&outpoint.vout),
        }
    }

    /// Returns a [`BTreeMap`] of vout to output of the provided `txid`.
    pub fn txouts(&self, txid: Txid) -> Option<BTreeMap<u32, &TxOut>> {
        Some(match &self.txs.get(&txid)?.0 {
            TxNode::Whole(tx) => tx
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
    /// the full transactions or individual txouts). If the returned value is negative, then the
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

    /// The transactions spending from this output.
    ///
    /// `TxGraph` allows conflicting transactions within the graph. Obviously the transactions in
    /// the returned set will never be in the same active-chain.
    pub fn outspends(&self, outpoint: OutPoint) -> &HashSet<Txid> {
        self.spends.get(&outpoint).unwrap_or(&self.empty_outspends)
    }

    /// Iterates over the transactions spending from `txid`.
    ///
    /// The iterator item is a union of `(vout, txid-set)` where:
    ///
    /// - `vout` is the provided `txid`'s outpoint that is being spent
    /// - `txid-set` is the set of txids spending the `vout`.
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
    pub fn partial_transactions(
        &self,
    ) -> impl Iterator<Item = TxInGraph<'_, BTreeMap<u32, TxOut>, A>> {
        self.txs
            .iter()
            .filter_map(|(&txid, (tx, anchors, last_seen))| match tx {
                TxNode::Whole(_) => None,
                TxNode::Partial(partial) => Some(TxInGraph {
                    txid,
                    tx: partial,
                    anchors,
                    last_seen: *last_seen,
                }),
            })
    }

    /// Creates an iterator that filters and maps descendants from the starting `txid`.
    ///
    /// The supplied closure takes in two inputs `(depth, descendant_txid)`:
    ///
    /// * `depth` is the distance between the starting `txid` and the `descendant_txid`. I.e., if the
    ///     descendant is spending an output of the starting `txid`; the `depth` will be 1.
    /// * `descendant_txid` is the descendant's txid which we are considering to walk.
    ///
    /// The supplied closure returns an `Option<T>`, allowing the caller to map each node it vists
    /// and decide whether to visit descendants.
    pub fn walk_descendants<'g, F, O>(&'g self, txid: Txid, walk_map: F) -> TxDescendants<A, F>
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
    ) -> TxDescendants<A, F>
    where
        F: FnMut(usize, Txid) -> Option<O> + 'g,
    {
        let txids = self.direct_conflicts_of_tx(tx).map(|(_, txid)| txid);
        TxDescendants::from_multiple_include_root(self, txids, walk_map)
    }

    /// Given a transaction, return an iterator of txids that directly conflict with the given
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
            .filter_map(move |(vin, txin)| self.spends.get(&txin.previous_output).zip(Some(vin)))
            .flat_map(|(spends, vin)| core::iter::repeat(vin).zip(spends.iter().cloned()))
            .filter(move |(_, conflicting_txid)| *conflicting_txid != txid)
    }

    /// Whether the graph has any transactions or outputs in it.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }
}

impl<A: Clone + Ord> TxGraph<A> {
    /// Construct a new [`TxGraph`] from a list of transactions.
    pub fn new(txs: impl IntoIterator<Item = Transaction>) -> Self {
        let mut new = Self::default();
        for tx in txs.into_iter() {
            let _ = new.insert_tx(tx);
        }
        new
    }

    /// Returns the resultant [`Additions`] if the given `txout` is inserted at `outpoint`. Does not
    /// mutate `self`.
    ///
    /// The [`Additions`] result will be empty if the `outpoint` (or a full transaction containing
    /// the `outpoint`) already existed in `self`.
    pub fn insert_txout_preview(&self, outpoint: OutPoint, txout: TxOut) -> Additions<A> {
        let mut update = Self::default();
        update.txs.insert(
            outpoint.txid,
            (
                TxNode::Partial([(outpoint.vout, txout)].into()),
                BTreeSet::new(),
                0,
            ),
        );
        self.determine_additions(&update)
    }

    /// Inserts the given [`TxOut`] at [`OutPoint`].
    ///
    /// Note this will ignore the action if we already have the full transaction that the txout is
    /// alleged to be on (even if it doesn't match it!).
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) -> Additions<A> {
        let additions = self.insert_txout_preview(outpoint, txout);
        self.apply_additions(additions.clone());
        additions
    }

    /// Returns the resultant [`Additions`] if the given transaction is inserted. Does not actually
    /// mutate [`Self`].
    ///
    /// The [`Additions`] result will be empty if `tx` already exists in `self`.
    pub fn insert_tx_preview(&self, tx: Transaction) -> Additions<A> {
        let mut update = Self::default();
        update
            .txs
            .insert(tx.txid(), (TxNode::Whole(tx), BTreeSet::new(), 0));
        self.determine_additions(&update)
    }

    /// Inserts the given transaction into [`TxGraph`].
    ///
    /// The [`Additions`] returned will be empty if `tx` already exists.
    pub fn insert_tx(&mut self, tx: Transaction) -> Additions<A> {
        let additions = self.insert_tx_preview(tx);
        self.apply_additions(additions.clone());
        additions
    }

    /// Returns the resultant [`Additions`] if the `txid` is set in `anchor`.
    pub fn insert_anchor_preview(&self, txid: Txid, anchor: A) -> Additions<A> {
        let mut update = Self::default();
        update.anchors.insert((anchor, txid));
        self.determine_additions(&update)
    }

    /// Inserts the given `anchor` into [`TxGraph`].
    ///
    /// This is equivalent to calling [`insert_anchor_preview`] and [`apply_additions`] in sequence.
    /// The [`Additions`] returned will be empty if graph already knows that `txid` exists in
    /// `anchor`.
    ///
    /// [`insert_anchor_preview`]: Self::insert_anchor_preview
    /// [`apply_additions`]: Self::apply_additions
    pub fn insert_anchor(&mut self, txid: Txid, anchor: A) -> Additions<A> {
        let additions = self.insert_anchor_preview(txid, anchor);
        self.apply_additions(additions.clone());
        additions
    }

    /// Returns the resultant [`Additions`] if the `txid` is set to `seen_at`.
    ///
    /// Note that [`TxGraph`] only keeps track of the lastest `seen_at`.
    pub fn insert_seen_at_preview(&self, txid: Txid, seen_at: u64) -> Additions<A> {
        let mut update = Self::default();
        let (_, _, update_last_seen) = update.txs.entry(txid).or_default();
        *update_last_seen = seen_at;
        self.determine_additions(&update)
    }

    /// Inserts the given `seen_at` into [`TxGraph`].
    ///
    /// This is equivalent to calling [`insert_seen_at_preview`] and [`apply_additions`] in
    /// sequence.
    ///
    /// [`insert_seen_at_preview`]: Self::insert_seen_at_preview
    /// [`apply_additions`]: Self::apply_additions
    pub fn insert_seen_at(&mut self, txid: Txid, seen_at: u64) -> Additions<A> {
        let additions = self.insert_seen_at_preview(txid, seen_at);
        self.apply_additions(additions.clone());
        additions
    }

    /// Extends this graph with another so that `self` becomes the union of the two sets of
    /// transactions.
    ///
    /// The returned [`Additions`] is the set difference between `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub fn apply_update(&mut self, update: TxGraph<A>) -> Additions<A> {
        let additions = self.determine_additions(&update);
        self.apply_additions(additions.clone());
        additions
    }

    /// Applies [`Additions`] to [`TxGraph`].
    pub fn apply_additions(&mut self, additions: Additions<A>) {
        for tx in additions.tx {
            let txid = tx.txid();

            tx.input
                .iter()
                .map(|txin| txin.previous_output)
                // coinbase spends are not to be counted
                .filter(|outpoint| !outpoint.is_null())
                // record spend as this tx has spent this outpoint
                .for_each(|outpoint| {
                    self.spends.entry(outpoint).or_default().insert(txid);
                });

            match self.txs.get_mut(&txid) {
                Some((tx_node @ TxNode::Partial(_), _, _)) => {
                    *tx_node = TxNode::Whole(tx);
                }
                Some((TxNode::Whole(tx), _, _)) => {
                    debug_assert_eq!(
                        tx.txid(),
                        txid,
                        "tx should produce txid that is same as key"
                    );
                }
                None => {
                    self.txs
                        .insert(txid, (TxNode::Whole(tx), BTreeSet::new(), 0));
                }
            }
        }

        for (outpoint, txout) in additions.txout {
            let tx_entry = self
                .txs
                .entry(outpoint.txid)
                .or_insert_with(Default::default);

            match tx_entry {
                (TxNode::Whole(_), _, _) => { /* do nothing since we already have full tx */ }
                (TxNode::Partial(txouts), _, _) => {
                    txouts.insert(outpoint.vout, txout);
                }
            }
        }

        for (anchor, txid) in additions.anchors {
            if self.anchors.insert((anchor.clone(), txid)) {
                let (_, anchors, _) = self.txs.entry(txid).or_insert_with(Default::default);
                anchors.insert(anchor);
            }
        }

        for (txid, new_last_seen) in additions.last_seen {
            let (_, _, last_seen) = self.txs.entry(txid).or_insert_with(Default::default);
            if new_last_seen > *last_seen {
                *last_seen = new_last_seen;
            }
        }
    }

    /// Previews the resultant [`Additions`] when [`Self`] is updated against the `update` graph.
    ///
    /// The [`Additions`] would be the set difference between `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub fn determine_additions(&self, update: &TxGraph<A>) -> Additions<A> {
        let mut additions = Additions::default();

        for (&txid, (update_tx_node, _, update_last_seen)) in &update.txs {
            let prev_last_seen: u64 = match (self.txs.get(&txid), update_tx_node) {
                (None, TxNode::Whole(update_tx)) => {
                    additions.tx.insert(update_tx.clone());
                    0
                }
                (None, TxNode::Partial(update_txos)) => {
                    additions.txout.extend(
                        update_txos
                            .iter()
                            .map(|(&vout, txo)| (OutPoint::new(txid, vout), txo.clone())),
                    );
                    0
                }
                (Some((TxNode::Whole(_), _, last_seen)), _) => *last_seen,
                (Some((TxNode::Partial(_), _, last_seen)), TxNode::Whole(update_tx)) => {
                    additions.tx.insert(update_tx.clone());
                    *last_seen
                }
                (Some((TxNode::Partial(txos), _, last_seen)), TxNode::Partial(update_txos)) => {
                    additions.txout.extend(
                        update_txos
                            .iter()
                            .filter(|(vout, _)| !txos.contains_key(*vout))
                            .map(|(&vout, txo)| (OutPoint::new(txid, vout), txo.clone())),
                    );
                    *last_seen
                }
            };

            if *update_last_seen > prev_last_seen {
                additions.last_seen.insert(txid, *update_last_seen);
            }
        }

        additions.anchors = update.anchors.difference(&self.anchors).cloned().collect();

        additions
    }
}

impl<A: BlockAnchor> TxGraph<A> {
    /// Get all heights that are relevant to the graph.
    pub fn relevant_heights(&self) -> BTreeSet<u32> {
        self.anchors
            .iter()
            .map(|(a, _)| a.anchor_block().height)
            .collect()
    }

    /// Determines whether a transaction of `txid` is in the best chain.
    ///
    /// TODO: Also return conflicting tx list, ordered by last_seen.
    pub fn try_get_chain_position<C>(
        &self,
        chain: C,
        txid: Txid,
    ) -> Result<Option<ObservedIn<&A>>, C::Error>
    where
        C: ChainOracle,
    {
        let (tx_node, anchors, &last_seen) = match self.txs.get(&txid) {
            Some((tx, anchors, last_seen)) if !(anchors.is_empty() && *last_seen == 0) => {
                (tx, anchors, last_seen)
            }
            _ => return Ok(None),
        };

        for anchor in anchors {
            if chain.is_block_in_best_chain(anchor.anchor_block())? {
                return Ok(Some(ObservedIn::Block(anchor)));
            }
        }

        // The tx is not anchored to a block which is in the best chain, let's check whether we can
        // ignore it by checking conflicts!
        let tx = match tx_node {
            TxNode::Whole(tx) => tx,
            TxNode::Partial(_) => {
                // [TODO] Unfortunately, we can't iterate over conflicts of partial txs right now!
                // [TODO] So we just assume the partial tx does not exist in the best chain :/
                return Ok(None);
            }
        };

        // [TODO] Is this logic correct? I do not think so, but it should be good enough for now!
        let mut latest_last_seen = 0_u64;
        for conflicting_tx in self.walk_conflicts(tx, |_, txid| self.get_tx(txid)) {
            for block_id in conflicting_tx.anchors.iter().map(A::anchor_block) {
                if chain.is_block_in_best_chain(block_id)? {
                    // conflicting tx is in best chain, so the current tx cannot be in best chain!
                    return Ok(None);
                }
            }
            if conflicting_tx.last_seen > latest_last_seen {
                latest_last_seen = conflicting_tx.last_seen;
            }
        }
        if last_seen >= latest_last_seen {
            Ok(Some(ObservedIn::Mempool(last_seen)))
        } else {
            Ok(None)
        }
    }

    pub fn get_chain_position<C>(&self, chain: C, txid: Txid) -> Option<ObservedIn<&A>>
    where
        C: ChainOracle<Error = Infallible>,
    {
        self.try_get_chain_position(chain, txid)
            .expect("error is infallible")
    }

    pub fn try_get_spend_in_chain<C>(
        &self,
        chain: C,
        outpoint: OutPoint,
    ) -> Result<Option<(ObservedIn<&A>, Txid)>, C::Error>
    where
        C: ChainOracle,
    {
        if self
            .try_get_chain_position(&chain, outpoint.txid)?
            .is_none()
        {
            return Ok(None);
        }
        if let Some(spends) = self.spends.get(&outpoint) {
            for &txid in spends {
                if let Some(observed_at) = self.try_get_chain_position(&chain, txid)? {
                    return Ok(Some((observed_at, txid)));
                }
            }
        }
        Ok(None)
    }

    pub fn get_chain_spend<C>(&self, chain: C, outpoint: OutPoint) -> Option<(ObservedIn<&A>, Txid)>
    where
        C: ChainOracle<Error = Infallible>,
    {
        self.try_get_spend_in_chain(chain, outpoint)
            .expect("error is infallible")
    }
}

/// A structure that represents changes to a [`TxGraph`].
///
/// It is named "additions" because [`TxGraph`] is monotone, so transactions can only be added and
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
            deserialize = "A: Ord + serde::Deserialize<'de>",
            serialize = "A: Ord + serde::Serialize",
        )
    )
)]
#[must_use]
pub struct Additions<A> {
    pub tx: BTreeSet<Transaction>,
    pub txout: BTreeMap<OutPoint, TxOut>,
    pub anchors: BTreeSet<(A, Txid)>,
    pub last_seen: BTreeMap<Txid, u64>,
}

impl<A> Default for Additions<A> {
    fn default() -> Self {
        Self {
            tx: Default::default(),
            txout: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
        }
    }
}

impl<A> Additions<A> {
    /// Returns true if the [`Additions`] is empty (no transactions or txouts).
    pub fn is_empty(&self) -> bool {
        self.tx.is_empty() && self.txout.is_empty()
    }

    /// Iterates over all outpoints contained within [`Additions`].
    pub fn txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.tx
            .iter()
            .flat_map(|tx| {
                tx.output
                    .iter()
                    .enumerate()
                    .map(move |(vout, txout)| (OutPoint::new(tx.txid(), vout as _), txout))
            })
            .chain(self.txout.iter().map(|(op, txout)| (*op, txout)))
    }

    /// Appends the changes in `other` into self such that applying `self` afterward has the same
    /// effect as sequentially applying the original `self` and `other`.
    pub fn append(&mut self, mut other: Additions<A>) {
        self.tx.append(&mut other.tx);
        self.txout.append(&mut other.txout);
    }
}

impl<A> AsRef<TxGraph<A>> for TxGraph<A> {
    fn as_ref(&self) -> &TxGraph<A> {
        self
    }
}

impl<A> ForEachTxOut for Additions<A> {
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut))) {
        self.txouts().for_each(f)
    }
}

impl<A> ForEachTxOut for TxGraph<A> {
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut))) {
        self.all_txouts().for_each(f)
    }
}

/// An iterator that traverses transaction descendants.
///
/// This `struct` is created by the [`walk_descendants`] method of [`TxGraph`].
///
/// [`walk_descendants`]: TxGraph::walk_descendants
pub struct TxDescendants<'g, A, F> {
    graph: &'g TxGraph<A>,
    visited: HashSet<Txid>,
    stack: Vec<(usize, Txid)>,
    filter_map: F,
}

impl<'g, A, F> TxDescendants<'g, A, F> {
    /// Creates a `TxDescendants` that includes the starting `txid` when iterating.
    #[allow(unused)]
    pub(crate) fn new_include_root(graph: &'g TxGraph<A>, txid: Txid, filter_map: F) -> Self {
        Self {
            graph,
            visited: Default::default(),
            stack: [(0, txid)].into(),
            filter_map,
        }
    }

    /// Creates a `TxDescendants` that excludes the starting `txid` when iterating.
    pub(crate) fn new_exclude_root(graph: &'g TxGraph<A>, txid: Txid, filter_map: F) -> Self {
        let mut descendants = Self {
            graph,
            visited: Default::default(),
            stack: Default::default(),
            filter_map,
        };
        descendants.populate_stack(1, txid);
        descendants
    }

    /// Creates a `TxDescendants` from multiple starting transactions that include the starting
    /// `txid`s when iterating.
    pub(crate) fn from_multiple_include_root<I>(
        graph: &'g TxGraph<A>,
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
        graph: &'g TxGraph<A>,
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

impl<'g, A, F> TxDescendants<'g, A, F> {
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

impl<'g, A, F, O> Iterator for TxDescendants<'g, A, F>
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
        Some(item)
    }
}

fn tx_outpoint_range(txid: Txid) -> RangeInclusive<OutPoint> {
    OutPoint::new(txid, u32::MIN)..=OutPoint::new(txid, u32::MAX)
}
