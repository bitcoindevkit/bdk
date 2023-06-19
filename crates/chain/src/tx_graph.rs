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
//! let mut graph: TxGraph = TxGraph::default();
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
//! let mut graph: TxGraph = TxGraph::default();
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

use crate::{
    collections::*, keychain::Balance, Anchor, Append, BlockId, ChainOracle, ChainPosition,
    ForEachTxOut, FullTxOut,
};
use alloc::vec::Vec;
use bitcoin::{OutPoint, Script, Transaction, TxOut, Txid};
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
    txs: HashMap<Txid, (TxNodeInternal, BTreeSet<A>, u64)>,
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

/// An outward-facing view of a (transaction) node in the [`TxGraph`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TxNode<'a, T, A> {
    /// Txid of the transaction.
    pub txid: Txid,
    /// A partial or full representation of the transaction.
    pub tx: &'a T,
    /// The blocks that the transaction is "anchored" in.
    pub anchors: &'a BTreeSet<A>,
    /// The last-seen unix timestamp of the transaction as unconfirmed.
    pub last_seen_unconfirmed: u64,
}

impl<'a, T, A> Deref for TxNode<'a, T, A> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.tx
    }
}

/// Internal representation of a transaction node of a [`TxGraph`].
///
/// This can either be a whole transaction, or a partial transaction (where we only have select
/// outputs).
#[derive(Clone, Debug, PartialEq)]
enum TxNodeInternal {
    Whole(Transaction),
    Partial(BTreeMap<u32, TxOut>),
}

impl Default for TxNodeInternal {
    fn default() -> Self {
        Self::Partial(BTreeMap::new())
    }
}

/// An outwards-facing view of a transaction that is part of the *best chain*'s history.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalTx<'a, T, A> {
    /// How the transaction is observed as (confirmed or unconfirmed).
    pub observed_as: ChainPosition<&'a A>,
    /// The transaction node (as part of the graph).
    pub node: TxNode<'a, T, A>,
}

impl<A> TxGraph<A> {
    /// Iterate over all tx outputs known by [`TxGraph`].
    ///
    /// This includes txouts of both full transactions as well as floating transactions.
    pub fn all_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs.iter().flat_map(|(txid, (tx, _, _))| match tx {
            TxNodeInternal::Whole(tx) => tx
                .output
                .iter()
                .enumerate()
                .map(|(vout, txout)| (OutPoint::new(*txid, vout as _), txout))
                .collect::<Vec<_>>(),
            TxNodeInternal::Partial(txouts) => txouts
                .iter()
                .map(|(vout, txout)| (OutPoint::new(*txid, *vout as _), txout))
                .collect::<Vec<_>>(),
        })
    }

    /// Iterate over floating txouts known by [`TxGraph`].
    ///
    /// Floating txouts are txouts that do not have the residing full transaction contained in the
    /// graph.
    pub fn floating_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs
            .iter()
            .filter_map(|(txid, (tx_node, _, _))| match tx_node {
                TxNodeInternal::Whole(_) => None,
                TxNodeInternal::Partial(txouts) => Some(
                    txouts
                        .iter()
                        .map(|(&vout, txout)| (OutPoint::new(*txid, vout), txout)),
                ),
            })
            .flatten()
    }

    /// Iterate over all full transactions in the graph.
    pub fn full_txs(&self) -> impl Iterator<Item = TxNode<'_, Transaction, A>> {
        self.txs
            .iter()
            .filter_map(|(&txid, (tx, anchors, last_seen))| match tx {
                TxNodeInternal::Whole(tx) => Some(TxNode {
                    txid,
                    tx,
                    anchors,
                    last_seen_unconfirmed: *last_seen,
                }),
                TxNodeInternal::Partial(_) => None,
            })
    }

    /// Get a transaction by txid. This only returns `Some` for full transactions.
    ///
    /// Refer to [`get_txout`] for getting a specific [`TxOut`].
    ///
    /// [`get_txout`]: Self::get_txout
    pub fn get_tx(&self, txid: Txid) -> Option<&Transaction> {
        self.get_tx_node(txid).map(|n| n.tx)
    }

    /// Get a transaction node by txid. This only returns `Some` for full transactions.
    pub fn get_tx_node(&self, txid: Txid) -> Option<TxNode<'_, Transaction, A>> {
        match &self.txs.get(&txid)? {
            (TxNodeInternal::Whole(tx), anchors, last_seen) => Some(TxNode {
                txid,
                tx,
                anchors,
                last_seen_unconfirmed: *last_seen,
            }),
            _ => None,
        }
    }

    /// Obtains a single tx output (if any) at the specified outpoint.
    pub fn get_txout(&self, outpoint: OutPoint) -> Option<&TxOut> {
        match &self.txs.get(&outpoint.txid)?.0 {
            TxNodeInternal::Whole(tx) => tx.output.get(outpoint.vout as usize),
            TxNodeInternal::Partial(txouts) => txouts.get(&outpoint.vout),
        }
    }

    /// Returns known outputs of a given `txid`.
    ///
    /// Returns a [`BTreeMap`] of vout to output of the provided `txid`.
    pub fn tx_outputs(&self, txid: Txid) -> Option<BTreeMap<u32, &TxOut>> {
        Some(match &self.txs.get(&txid)?.0 {
            TxNodeInternal::Whole(tx) => tx
                .output
                .iter()
                .enumerate()
                .map(|(vout, txout)| (vout as u32, txout))
                .collect::<BTreeMap<_, _>>(),
            TxNodeInternal::Partial(txouts) => txouts
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
    pub fn tx_spends(
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

    /// Get all transaction anchors known by [`TxGraph`].
    pub fn all_anchors(&self) -> &BTreeSet<(A, Txid)> {
        &self.anchors
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
    /// Inserting floating txouts are useful for determining fee/feerate of transactions we care
    /// about.
    ///
    /// The [`Additions`] result will be empty if the `outpoint` (or a full transaction containing
    /// the `outpoint`) already existed in `self`.
    pub fn insert_txout_preview(&self, outpoint: OutPoint, txout: TxOut) -> Additions<A> {
        let mut update = Self::default();
        update.txs.insert(
            outpoint.txid,
            (
                TxNodeInternal::Partial([(outpoint.vout, txout)].into()),
                BTreeSet::new(),
                0,
            ),
        );
        self.determine_additions(&update)
    }

    /// Inserts the given [`TxOut`] at [`OutPoint`].
    ///
    /// This is equivalent to calling [`insert_txout_preview`] and [`apply_additions`] in sequence.
    ///
    /// [`insert_txout_preview`]: Self::insert_txout_preview
    /// [`apply_additions`]: Self::apply_additions
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
            .insert(tx.txid(), (TxNodeInternal::Whole(tx), BTreeSet::new(), 0));
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
        for tx in additions.txs {
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
                Some((tx_node @ TxNodeInternal::Partial(_), _, _)) => {
                    *tx_node = TxNodeInternal::Whole(tx);
                }
                Some((TxNodeInternal::Whole(tx), _, _)) => {
                    debug_assert_eq!(
                        tx.txid(),
                        txid,
                        "tx should produce txid that is same as key"
                    );
                }
                None => {
                    self.txs
                        .insert(txid, (TxNodeInternal::Whole(tx), BTreeSet::new(), 0));
                }
            }
        }

        for (outpoint, txout) in additions.txouts {
            let tx_entry = self
                .txs
                .entry(outpoint.txid)
                .or_insert_with(Default::default);

            match tx_entry {
                (TxNodeInternal::Whole(_), _, _) => { /* do nothing since we already have full tx */
                }
                (TxNodeInternal::Partial(txouts), _, _) => {
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
                (None, TxNodeInternal::Whole(update_tx)) => {
                    additions.txs.insert(update_tx.clone());
                    0
                }
                (None, TxNodeInternal::Partial(update_txos)) => {
                    additions.txouts.extend(
                        update_txos
                            .iter()
                            .map(|(&vout, txo)| (OutPoint::new(txid, vout), txo.clone())),
                    );
                    0
                }
                (Some((TxNodeInternal::Whole(_), _, last_seen)), _) => *last_seen,
                (
                    Some((TxNodeInternal::Partial(_), _, last_seen)),
                    TxNodeInternal::Whole(update_tx),
                ) => {
                    additions.txs.insert(update_tx.clone());
                    *last_seen
                }
                (
                    Some((TxNodeInternal::Partial(txos), _, last_seen)),
                    TxNodeInternal::Partial(update_txos),
                ) => {
                    additions.txouts.extend(
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

impl<A: Anchor> TxGraph<A> {
    /// Get the position of the transaction in `chain` with tip `chain_tip`.
    ///
    /// If the given transaction of `txid` does not exist in the chain of `chain_tip`, `None` is
    /// returned.
    ///
    /// # Error
    ///
    /// An error will occur if the [`ChainOracle`] implementation (`chain`) fails. If the
    /// [`ChainOracle`] is infallible, [`get_chain_position`] can be used instead.
    ///
    /// [`get_chain_position`]: Self::get_chain_position
    pub fn try_get_chain_position<C: ChainOracle>(
        &self,
        chain: &C,
        chain_tip: BlockId,
        txid: Txid,
    ) -> Result<Option<ChainPosition<&A>>, C::Error> {
        let (tx_node, anchors, last_seen) = match self.txs.get(&txid) {
            Some(v) => v,
            None => return Ok(None),
        };

        for anchor in anchors {
            match chain.is_block_in_chain(anchor.anchor_block(), chain_tip)? {
                Some(true) => return Ok(Some(ChainPosition::Confirmed(anchor))),
                _ => continue,
            }
        }

        // The tx is not anchored to a block which is in the best chain, let's check whether we can
        // ignore it by checking conflicts!
        let tx = match tx_node {
            TxNodeInternal::Whole(tx) => tx,
            TxNodeInternal::Partial(_) => {
                // Partial transactions (outputs only) cannot have conflicts.
                return Ok(None);
            }
        };

        // If a conflicting tx is in the best chain, or has `last_seen` higher than this tx, then
        // this tx cannot exist in the best chain
        for conflicting_tx in self.walk_conflicts(tx, |_, txid| self.get_tx_node(txid)) {
            for block in conflicting_tx.anchors.iter().map(A::anchor_block) {
                if chain.is_block_in_chain(block, chain_tip)? == Some(true) {
                    // conflicting tx is in best chain, so the current tx cannot be in best chain!
                    return Ok(None);
                }
            }
            if conflicting_tx.last_seen_unconfirmed > *last_seen {
                return Ok(None);
            }
        }

        Ok(Some(ChainPosition::Unconfirmed(*last_seen)))
    }

    /// Get the position of the transaction in `chain` with tip `chain_tip`.
    ///
    /// This is the infallible version of [`try_get_chain_position`].
    ///
    /// [`try_get_chain_position`]: Self::try_get_chain_position
    pub fn get_chain_position<C: ChainOracle<Error = Infallible>>(
        &self,
        chain: &C,
        chain_tip: BlockId,
        txid: Txid,
    ) -> Option<ChainPosition<&A>> {
        self.try_get_chain_position(chain, chain_tip, txid)
            .expect("error is infallible")
    }

    /// Get the txid of the spending transaction and where the spending transaction is observed in
    /// the `chain` of `chain_tip`.
    ///
    /// If no in-chain transaction spends `outpoint`, `None` will be returned.
    ///
    /// # Error
    ///
    /// An error will occur only if the [`ChainOracle`] implementation (`chain`) fails.
    ///
    /// If the [`ChainOracle`] is infallible, [`get_chain_spend`] can be used instead.
    ///
    /// [`get_chain_spend`]: Self::get_chain_spend
    pub fn try_get_chain_spend<C: ChainOracle>(
        &self,
        chain: &C,
        chain_tip: BlockId,
        outpoint: OutPoint,
    ) -> Result<Option<(ChainPosition<&A>, Txid)>, C::Error> {
        if self
            .try_get_chain_position(chain, chain_tip, outpoint.txid)?
            .is_none()
        {
            return Ok(None);
        }
        if let Some(spends) = self.spends.get(&outpoint) {
            for &txid in spends {
                if let Some(observed_at) = self.try_get_chain_position(chain, chain_tip, txid)? {
                    return Ok(Some((observed_at, txid)));
                }
            }
        }
        Ok(None)
    }

    /// Get the txid of the spending transaction and where the spending transaction is observed in
    /// the `chain` of `chain_tip`.
    ///
    /// This is the infallible version of [`try_get_chain_spend`]
    ///
    /// [`try_get_chain_spend`]: Self::try_get_chain_spend
    pub fn get_chain_spend<C: ChainOracle<Error = Infallible>>(
        &self,
        chain: &C,
        static_block: BlockId,
        outpoint: OutPoint,
    ) -> Option<(ChainPosition<&A>, Txid)> {
        self.try_get_chain_spend(chain, static_block, outpoint)
            .expect("error is infallible")
    }

    /// List graph transactions that are in `chain` with `chain_tip`.
    ///
    /// Each transaction is represented as a [`CanonicalTx`] that contains where the transaction is
    /// observed in-chain, and the [`TxNode`].
    ///
    /// # Error
    ///
    /// If the [`ChainOracle`] implementation (`chain`) fails, an error will be returned with the
    /// returned item.
    ///
    /// If the [`ChainOracle`] is infallible, [`list_chain_txs`] can be used instead.
    ///
    /// [`list_chain_txs`]: Self::list_chain_txs
    pub fn try_list_chain_txs<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<CanonicalTx<'a, Transaction, A>, C::Error>> {
        self.full_txs().filter_map(move |tx| {
            self.try_get_chain_position(chain, chain_tip, tx.txid)
                .map(|v| {
                    v.map(|observed_in| CanonicalTx {
                        observed_as: observed_in,
                        node: tx,
                    })
                })
                .transpose()
        })
    }

    /// List graph transactions that are in `chain` with `chain_tip`.
    ///
    /// This is the infallible version of [`try_list_chain_txs`].
    ///
    /// [`try_list_chain_txs`]: Self::try_list_chain_txs
    pub fn list_chain_txs<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = CanonicalTx<'a, Transaction, A>> {
        self.try_list_chain_txs(chain, chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }

    /// Get a filtered list of outputs from the given `outpoints` that are in `chain` with
    /// `chain_tip`.
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`OI`) for convenience. If `OI` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    ///
    /// Floating outputs are ignored.
    ///
    /// # Error
    ///
    /// An [`Iterator::Item`] can be an [`Err`] if the [`ChainOracle`] implementation (`chain`)
    /// fails.
    ///
    /// If the [`ChainOracle`] implementation is infallible, [`filter_chain_txouts`] can be used
    /// instead.
    ///
    /// [`filter_chain_txouts`]: Self::filter_chain_txouts
    pub fn try_filter_chain_txouts<'a, C: ChainOracle + 'a, OI: Clone + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
        outpoints: impl IntoIterator<Item = (OI, OutPoint)> + 'a,
    ) -> impl Iterator<Item = Result<(OI, FullTxOut<A>), C::Error>> + 'a {
        outpoints
            .into_iter()
            .map(
                move |(spk_i, op)| -> Result<Option<(OI, FullTxOut<_>)>, C::Error> {
                    let tx_node = match self.get_tx_node(op.txid) {
                        Some(n) => n,
                        None => return Ok(None),
                    };

                    let txout = match tx_node.tx.output.get(op.vout as usize) {
                        Some(txout) => txout.clone(),
                        None => return Ok(None),
                    };

                    let chain_position =
                        match self.try_get_chain_position(chain, chain_tip, op.txid)? {
                            Some(pos) => pos.cloned(),
                            None => return Ok(None),
                        };

                    let spent_by = self
                        .try_get_chain_spend(chain, chain_tip, op)?
                        .map(|(a, txid)| (a.cloned(), txid));

                    Ok(Some((
                        spk_i,
                        FullTxOut {
                            outpoint: op,
                            txout,
                            chain_position,
                            spent_by,
                            is_on_coinbase: tx_node.tx.is_coin_base(),
                        },
                    )))
                },
            )
            .filter_map(Result::transpose)
    }

    /// Get a filtered list of outputs from the given `outpoints` that are in `chain` with
    /// `chain_tip`.
    ///
    /// This is the infallible version of [`try_filter_chain_txouts`].
    ///
    /// [`try_filter_chain_txouts`]: Self::try_filter_chain_txouts
    pub fn filter_chain_txouts<'a, C: ChainOracle<Error = Infallible> + 'a, OI: Clone + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
        outpoints: impl IntoIterator<Item = (OI, OutPoint)> + 'a,
    ) -> impl Iterator<Item = (OI, FullTxOut<A>)> + 'a {
        self.try_filter_chain_txouts(chain, chain_tip, outpoints)
            .map(|r| r.expect("oracle is infallible"))
    }

    /// Get a filtered list of unspent outputs (UTXOs) from the given `outpoints` that are in
    /// `chain` with `chain_tip`.
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`OI`) for convenience. If `OI` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    ///
    /// Floating outputs are ignored.
    ///
    /// # Error
    ///
    /// An [`Iterator::Item`] can be an [`Err`] if the [`ChainOracle`] implementation (`chain`)
    /// fails.
    ///
    /// If the [`ChainOracle`] implementation is infallible, [`filter_chain_unspents`] can be used
    /// instead.
    ///
    /// [`filter_chain_unspents`]: Self::filter_chain_unspents
    pub fn try_filter_chain_unspents<'a, C: ChainOracle + 'a, OI: Clone + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
        outpoints: impl IntoIterator<Item = (OI, OutPoint)> + 'a,
    ) -> impl Iterator<Item = Result<(OI, FullTxOut<A>), C::Error>> + 'a {
        self.try_filter_chain_txouts(chain, chain_tip, outpoints)
            .filter(|r| match r {
                // keep unspents, drop spents
                Ok((_, full_txo)) => full_txo.spent_by.is_none(),
                // keep errors
                Err(_) => true,
            })
    }

    /// Get a filtered list of unspent outputs (UTXOs) from the given `outpoints` that are in
    /// `chain` with `chain_tip`.
    ///
    /// This is the infallible version of [`try_filter_chain_unspents`].
    ///
    /// [`try_filter_chain_unspents`]: Self::try_filter_chain_unspents
    pub fn filter_chain_unspents<'a, C: ChainOracle<Error = Infallible> + 'a, OI: Clone + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
        txouts: impl IntoIterator<Item = (OI, OutPoint)> + 'a,
    ) -> impl Iterator<Item = (OI, FullTxOut<A>)> + 'a {
        self.try_filter_chain_unspents(chain, chain_tip, txouts)
            .map(|r| r.expect("oracle is infallible"))
    }

    /// Get the total balance of `outpoints` that are in `chain` of `chain_tip`.
    ///
    /// The output of `trust_predicate` should return `true` for scripts that we trust.
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`OI`) for convenience. If `OI` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    ///
    /// If the provided [`ChainOracle`] implementation (`chain`) is infallible, [`balance`] can be
    /// used instead.
    ///
    /// [`balance`]: Self::balance
    pub fn try_balance<C: ChainOracle, OI: Clone>(
        &self,
        chain: &C,
        chain_tip: BlockId,
        outpoints: impl IntoIterator<Item = (OI, OutPoint)>,
        mut trust_predicate: impl FnMut(&OI, &Script) -> bool,
    ) -> Result<Balance, C::Error> {
        let mut immature = 0;
        let mut trusted_pending = 0;
        let mut untrusted_pending = 0;
        let mut confirmed = 0;

        for res in self.try_filter_chain_unspents(chain, chain_tip, outpoints) {
            let (spk_i, txout) = res?;

            match &txout.chain_position {
                ChainPosition::Confirmed(_) => {
                    if txout.is_confirmed_and_spendable(chain_tip.height) {
                        confirmed += txout.txout.value;
                    } else if !txout.is_mature(chain_tip.height) {
                        immature += txout.txout.value;
                    }
                }
                ChainPosition::Unconfirmed(_) => {
                    if trust_predicate(&spk_i, &txout.txout.script_pubkey) {
                        trusted_pending += txout.txout.value;
                    } else {
                        untrusted_pending += txout.txout.value;
                    }
                }
            }
        }

        Ok(Balance {
            immature,
            trusted_pending,
            untrusted_pending,
            confirmed,
        })
    }

    /// Get the total balance of `outpoints` that are in `chain` of `chain_tip`.
    ///
    /// This is the infallible version of [`try_balance`].
    ///
    /// [`try_balance`]: Self::try_balance
    pub fn balance<C: ChainOracle<Error = Infallible>, OI: Clone>(
        &self,
        chain: &C,
        chain_tip: BlockId,
        outpoints: impl IntoIterator<Item = (OI, OutPoint)>,
        trust_predicate: impl FnMut(&OI, &Script) -> bool,
    ) -> Balance {
        self.try_balance(chain, chain_tip, outpoints, trust_predicate)
            .expect("oracle is infallible")
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
pub struct Additions<A = ()> {
    /// Added transactions.
    pub txs: BTreeSet<Transaction>,
    /// Added txouts.
    pub txouts: BTreeMap<OutPoint, TxOut>,
    /// Added anchors.
    pub anchors: BTreeSet<(A, Txid)>,
    /// Added last-seen unix timestamps of transactions.
    pub last_seen: BTreeMap<Txid, u64>,
}

impl<A> Default for Additions<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            txouts: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
        }
    }
}

impl<A> Additions<A> {
    /// Returns true if the [`Additions`] is empty (no transactions or txouts).
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty() && self.txouts.is_empty()
    }

    /// Iterates over all outpoints contained within [`Additions`].
    pub fn txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs
            .iter()
            .flat_map(|tx| {
                tx.output
                    .iter()
                    .enumerate()
                    .map(move |(vout, txout)| (OutPoint::new(tx.txid(), vout as _), txout))
            })
            .chain(self.txouts.iter().map(|(op, txout)| (*op, txout)))
    }
}

impl<A: Ord> Append for Additions<A> {
    fn append(&mut self, mut other: Self) {
        self.txs.append(&mut other.txs);
        self.txouts.append(&mut other.txouts);
        self.anchors.append(&mut other.anchors);

        // last_seen timestamps should only increase
        self.last_seen.extend(
            other
                .last_seen
                .into_iter()
                .filter(|(txid, update_ls)| self.last_seen.get(txid) < Some(update_ls))
                .collect::<Vec<_>>(),
        );
    }

    fn is_empty(&self) -> bool {
        self.txs.is_empty()
            && self.txouts.is_empty()
            && self.anchors.is_empty()
            && self.last_seen.is_empty()
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
