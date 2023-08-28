//! Module for structures that store and traverse transactions.
//!
//! [`TxGraph`] is a monotone structure that inserts transactions and indexes the spends. The
//! [`ChangeSet`] structure reports changes of [`TxGraph`] but can also be applied to a
//! [`TxGraph`] as well. Lastly, [`TxDescendants`] is an [`Iterator`] that traverses descendants of
//! a given transaction.
//!
//! Conflicting transactions are allowed to coexist within a [`TxGraph`]. This is useful for
//! identifying and traversing conflicts and descendants of a given transaction.
//!
//! # Applying changes
//!
//! Methods that apply changes to [`TxGraph`] will return [`ChangeSet`].
//! [`ChangeSet`] can be applied back to a [`TxGraph`] or be used to inform persistent storage
//! of the changes to [`TxGraph`].
//!
//! ```
//! # use bdk_chain::BlockId;
//! # use bdk_chain::tx_graph::TxGraph;
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! let mut graph: TxGraph = TxGraph::default();
//! let mut another_graph: TxGraph = TxGraph::default();
//!
//! // insert a transaction
//! let changeset = graph.insert_tx(tx_a);
//!
//! // the resulting changeset can be applied to another tx graph
//! another_graph.apply_changeset(changeset);
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
//! // apply the update graph
//! let changeset = graph.apply_update(update.clone());
//!
//! // if we apply it again, the resulting changeset will be empty
//! let changeset = graph.apply_update(update);
//! assert!(changeset.is_empty());
//! ```

use crate::{
    collections::*, keychain::Balance, local_chain::LocalChain, Anchor, Append, BlockId,
    ChainOracle, ChainPosition, ForEachTxOut, FullTxOut,
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
    pub chain_position: ChainPosition<&'a A>,
    /// The transaction node (as part of the graph).
    pub tx_node: TxNode<'a, T, A>,
}

/// Errors returned by `TxGraph::calculate_fee`.
#[derive(Debug, PartialEq, Eq)]
pub enum CalculateFeeError {
    /// Missing `TxOut` for one or more of the inputs of the tx
    MissingTxOut(Vec<OutPoint>),
    /// When the transaction is invalid according to the graph it has a negative fee
    NegativeFee(i64),
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
    /// Returns `OK(_)` if we have all the [`TxOut`]s being spent by `tx` in the graph (either as
    /// the full transactions or individual txouts).
    ///
    /// To calculate the fee for a [`Transaction`] that depends on foreign [`TxOut`] values you must
    /// first manually insert the foreign TxOuts into the tx graph using the [`insert_txout`] function.
    /// Only insert TxOuts you trust the values for!
    ///
    /// Note `tx` does not have to be in the graph for this to work.
    ///
    /// [`insert_txout`]: Self::insert_txout
    pub fn calculate_fee(&self, tx: &Transaction) -> Result<u64, CalculateFeeError> {
        if tx.is_coin_base() {
            return Ok(0);
        }

        let (inputs_sum, missing_outputs) = tx.input.iter().fold(
            (0_i64, Vec::new()),
            |(mut sum, mut missing_outpoints), txin| match self.get_txout(txin.previous_output) {
                None => {
                    missing_outpoints.push(txin.previous_output);
                    (sum, missing_outpoints)
                }
                Some(txout) => {
                    sum += txout.value as i64;
                    (sum, missing_outpoints)
                }
            },
        );
        if !missing_outputs.is_empty() {
            return Err(CalculateFeeError::MissingTxOut(missing_outputs));
        }

        let outputs_sum = tx
            .output
            .iter()
            .map(|txout| txout.value as i64)
            .sum::<i64>();

        let fee = inputs_sum - outputs_sum;
        if fee < 0 {
            Err(CalculateFeeError::NegativeFee(fee))
        } else {
            Ok(fee as u64)
        }
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
    /// ancestors and descendants of directly-conflicting transactions, which are also considered
    /// conflicts).
    ///
    /// Refer to [`Self::walk_descendants`] for `walk_map` usage.
    pub fn walk_conflicts<'g, F, O>(
        &'g self,
        tx: &'g Transaction,
        depth_limit: usize,
        walk_map: F,
    ) -> TxDescendants<A, F>
    where
        F: FnMut(usize, Txid) -> Option<O> + 'g,
    {
        let mut txids = HashSet::new();
        for (_, tx) in TxAncestors::new_include_root(self, tx.clone(), depth_limit) {
            txids.extend(self.direct_conflicts_of_tx(&tx).map(|(_, txid)| txid));
        }
        TxDescendants::from_multiple_include_root(self, txids, walk_map)
    }

    /// Given a transaction, return an iterator of txids that directly conflict with the given
    /// transaction's inputs (spends). The conflicting txids are returned with the given
    /// transaction's vin (in which it conflicts).
    ///
    /// Note that this only returns directly conflicting txids and does not include descendants of
    /// those txids or conflicting txids of the given transaction's ancestors (which are technically
    /// also conflicting).
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

    /// Inserts the given [`TxOut`] at [`OutPoint`].
    ///
    /// Inserting floating txouts are useful for determining fee/feerate of transactions we care
    /// about.
    ///
    /// The [`ChangeSet`] result will be empty if the `outpoint` (or a full transaction containing
    /// the `outpoint`) already existed in `self`.
    ///
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) -> ChangeSet<A> {
        let mut update = Self::default();
        update.txs.insert(
            outpoint.txid,
            (
                TxNodeInternal::Partial([(outpoint.vout, txout)].into()),
                BTreeSet::new(),
                0,
            ),
        );
        self.apply_update(update)
    }

    /// Inserts the given transaction into [`TxGraph`].
    ///
    /// The [`ChangeSet`] returned will be empty if `tx` already exists.
    pub fn insert_tx(&mut self, tx: Transaction) -> ChangeSet<A> {
        let mut update = Self::default();
        update
            .txs
            .insert(tx.txid(), (TxNodeInternal::Whole(tx), BTreeSet::new(), 0));
        self.apply_update(update)
    }

    /// Inserts the given `anchor` into [`TxGraph`].
    ///
    /// The [`ChangeSet`] returned will be empty if graph already knows that `txid` exists in
    /// `anchor`.
    pub fn insert_anchor(&mut self, txid: Txid, anchor: A) -> ChangeSet<A> {
        let mut update = Self::default();
        update.anchors.insert((anchor, txid));
        self.apply_update(update)
    }

    /// Inserts the given `seen_at` for `txid` into [`TxGraph`].
    ///
    /// Note that [`TxGraph`] only keeps track of the lastest `seen_at`.
    pub fn insert_seen_at(&mut self, txid: Txid, seen_at: u64) -> ChangeSet<A> {
        let mut update = Self::default();
        let (_, _, update_last_seen) = update.txs.entry(txid).or_default();
        *update_last_seen = seen_at;
        self.apply_update(update)
    }

    /// Extends this graph with another so that `self` becomes the union of the two sets of
    /// transactions.
    ///
    /// The returned [`ChangeSet`] is the set difference between `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub fn apply_update(&mut self, update: TxGraph<A>) -> ChangeSet<A> {
        let changeset = self.determine_changeset(update);
        self.apply_changeset(changeset.clone());
        changeset
    }

    /// Determines the [`ChangeSet`] between `self` and an empty [`TxGraph`].
    pub fn initial_changeset(&self) -> ChangeSet<A> {
        Self::default().determine_changeset(self.clone())
    }

    /// Applies [`ChangeSet`] to [`TxGraph`].
    pub fn apply_changeset(&mut self, changeset: ChangeSet<A>) {
        for tx in changeset.txs {
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

        for (outpoint, txout) in changeset.txouts {
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

        for (anchor, txid) in changeset.anchors {
            if self.anchors.insert((anchor.clone(), txid)) {
                let (_, anchors, _) = self.txs.entry(txid).or_insert_with(Default::default);
                anchors.insert(anchor);
            }
        }

        for (txid, new_last_seen) in changeset.last_seen {
            let (_, _, last_seen) = self.txs.entry(txid).or_insert_with(Default::default);
            if new_last_seen > *last_seen {
                *last_seen = new_last_seen;
            }
        }
    }

    /// Previews the resultant [`ChangeSet`] when [`Self`] is updated against the `update` graph.
    ///
    /// The [`ChangeSet`] would be the set difference between `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub(crate) fn determine_changeset(&self, update: TxGraph<A>) -> ChangeSet<A> {
        let mut changeset = ChangeSet::default();

        for (&txid, (update_tx_node, _, update_last_seen)) in &update.txs {
            let prev_last_seen: u64 = match (self.txs.get(&txid), update_tx_node) {
                (None, TxNodeInternal::Whole(update_tx)) => {
                    changeset.txs.insert(update_tx.clone());
                    0
                }
                (None, TxNodeInternal::Partial(update_txos)) => {
                    changeset.txouts.extend(
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
                    changeset.txs.insert(update_tx.clone());
                    *last_seen
                }
                (
                    Some((TxNodeInternal::Partial(txos), _, last_seen)),
                    TxNodeInternal::Partial(update_txos),
                ) => {
                    changeset.txouts.extend(
                        update_txos
                            .iter()
                            .filter(|(vout, _)| !txos.contains_key(*vout))
                            .map(|(&vout, txo)| (OutPoint::new(txid, vout), txo.clone())),
                    );
                    *last_seen
                }
            };

            if *update_last_seen > prev_last_seen {
                changeset.last_seen.insert(txid, *update_last_seen);
            }
        }

        changeset.anchors = update.anchors.difference(&self.anchors).cloned().collect();

        changeset
    }
}

impl<A: Anchor> TxGraph<A> {
    /// Find missing block heights of `chain`.
    ///
    /// This works by scanning through anchors, and seeing whether the anchor block of the anchor
    /// exists in the [`LocalChain`]. The returned iterator does not output duplicate heights.
    pub fn missing_heights<'a>(&'a self, chain: &'a LocalChain) -> impl Iterator<Item = u32> + 'a {
        // Map of txids to skip.
        //
        // Usually, if a height of a tx anchor is missing from the chain, we would want to return
        // this height in the iterator. The exception is when the tx is confirmed in chain. All the
        // other missing-height anchors of this tx can be skipped.
        //
        // * Some(true)  => skip all anchors of this txid
        // * Some(false) => do not skip anchors of this txid
        // * None        => we do not know whether we can skip this txid
        let mut txids_to_skip = HashMap::<Txid, bool>::new();

        // Keeps track of the last height emitted so we don't double up.
        let mut last_height_emitted = Option::<u32>::None;

        self.anchors
            .iter()
            .filter(move |(_, txid)| {
                let skip = *txids_to_skip.entry(*txid).or_insert_with(|| {
                    let tx_anchors = match self.txs.get(txid) {
                        Some((_, anchors, _)) => anchors,
                        None => return true,
                    };
                    let mut has_missing_height = false;
                    for anchor_block in tx_anchors.iter().map(Anchor::anchor_block) {
                        match chain.blocks().get(&anchor_block.height) {
                            None => {
                                has_missing_height = true;
                                continue;
                            }
                            Some(chain_hash) => {
                                if chain_hash == &anchor_block.hash {
                                    return true;
                                }
                            }
                        }
                    }
                    !has_missing_height
                });
                #[cfg(feature = "std")]
                debug_assert!({
                    println!("txid={} skip={}", txid, skip);
                    true
                });
                !skip
            })
            .filter_map(move |(a, _)| {
                let anchor_block = a.anchor_block();
                if Some(anchor_block.height) != last_height_emitted
                    && !chain.blocks().contains_key(&anchor_block.height)
                {
                    last_height_emitted = Some(anchor_block.height);
                    Some(anchor_block.height)
                } else {
                    None
                }
            })
    }

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
            TxNodeInternal::Whole(tx) => tx, // check_tx_conflicts.push((tx, last_seen, 0)),
            TxNodeInternal::Partial(_) => {
                // Partial transactions (outputs only) cannot have conflicts.
                return Ok(None);
            }
        };

        // If a conflicting tx is in the best chain, or has `last_seen` higher than this tx, then
        // this tx cannot exist in the best chain
        for conflicting_tx in self.walk_conflicts(tx, 25, |_, txid| self.get_tx_node(txid)) {
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
                        chain_position: observed_in,
                        tx_node: tx,
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
/// Since [`TxGraph`] is monotone "changeset" can only contain transactions to be added and
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
pub struct ChangeSet<A = ()> {
    /// Added transactions.
    pub txs: BTreeSet<Transaction>,
    /// Added txouts.
    pub txouts: BTreeMap<OutPoint, TxOut>,
    /// Added anchors.
    pub anchors: BTreeSet<(A, Txid)>,
    /// Added last-seen unix timestamps of transactions.
    pub last_seen: BTreeMap<Txid, u64>,
}

impl<A> Default for ChangeSet<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            txouts: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
        }
    }
}

impl<A> ChangeSet<A> {
    /// Returns true if the [`ChangeSet`] is empty (no transactions or txouts).
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty() && self.txouts.is_empty()
    }

    /// Iterates over all outpoints contained within [`ChangeSet`].
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

    /// Iterates over the heights of that the new transaction anchors in this changeset.
    ///
    /// This is useful if you want to find which heights you need to fetch data about in order to
    /// confirm or exclude these anchors.
    ///
    /// See also: [`TxGraph::missing_heights`]
    pub fn anchor_heights(&self) -> impl Iterator<Item = u32> + '_
    where
        A: Anchor,
    {
        let mut dedup = None;
        self.anchors
            .iter()
            .map(|(a, _)| a.anchor_block().height)
            .filter(move |height| {
                let duplicate = dedup == Some(*height);
                dedup = Some(*height);
                !duplicate
            })
    }

    /// Returns an iterator for the [`anchor_heights`] in this changeset that are not included in
    /// `local_chain`. This tells you which heights you need to include in `local_chain` in order
    /// for it to conclusively act as a [`ChainOracle`] for the transaction anchors this changeset
    /// will add.
    ///
    /// [`ChainOracle`]: crate::ChainOracle
    /// [`anchor_heights`]: Self::anchor_heights
    pub fn missing_heights_from<'a>(
        &'a self,
        local_chain: &'a LocalChain,
    ) -> impl Iterator<Item = u32> + 'a
    where
        A: Anchor,
    {
        self.anchor_heights()
            .filter(move |height| !local_chain.blocks().contains_key(height))
    }
}

impl<A: Ord> Append for ChangeSet<A> {
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

impl<A> ForEachTxOut for ChangeSet<A> {
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut))) {
        self.txouts().for_each(f)
    }
}

impl<A> ForEachTxOut for TxGraph<A> {
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut))) {
        self.all_txouts().for_each(f)
    }
}

/// An iterator that traverses transaction ancestors to a specified depth.
pub struct TxAncestors<'g, A> {
    graph: &'g TxGraph<A>,
    visited: HashSet<Txid>,
    stack: Vec<(usize, Transaction)>,
    depth_limit: usize,
}

impl<'g, A> TxAncestors<'g, A> {
    /// Creates a `TxAncestors` that includes the starting `Transaction` when iterating.
    pub(crate) fn new_include_root(
        graph: &'g TxGraph<A>,
        tx: Transaction,
        depth_limit: usize,
    ) -> Self {
        Self {
            graph,
            visited: Default::default(),
            stack: [(0, tx)].into(),
            depth_limit,
        }
    }

    /// Creates a `TxAncestors` that excludes the starting `Transaction` when iterating.
    #[allow(unused)]
    pub(crate) fn new_exclude_root(
        graph: &'g TxGraph<A>,
        tx: Transaction,
        depth_limit: usize,
    ) -> Self {
        let mut ancestors = Self {
            graph,
            visited: Default::default(),
            stack: Default::default(),
            depth_limit,
        };
        ancestors.populate_stack(1, tx);
        ancestors
    }

    /// Creates a `TxAncestors` from multiple starting `Transaction`s that includes the starting
    /// `Transaction`s when iterating.
    #[allow(unused)]
    pub(crate) fn from_multiple_include_root<I>(
        graph: &'g TxGraph<A>,
        txs: I,
        depth_limit: usize,
    ) -> Self
    where
        I: IntoIterator<Item = Transaction>,
    {
        Self {
            graph,
            visited: Default::default(),
            stack: txs.into_iter().map(|tx| (0, tx)).collect(),
            depth_limit,
        }
    }

    /// Creates a `TxAncestors` from multiple starting `Transaction`s that excludes the starting
    /// `Transaction`s when iterating.
    #[allow(unused)]
    pub(crate) fn from_multiple_exclude_root<I>(
        graph: &'g TxGraph<A>,
        txs: I,
        depth_limit: usize,
    ) -> Self
    where
        I: IntoIterator<Item = Transaction>,
    {
        let mut ancestors = Self {
            graph,
            visited: Default::default(),
            stack: Default::default(),
            depth_limit,
        };
        for tx in txs {
            ancestors.populate_stack(1, tx);
        }
        ancestors
    }
}

impl<'g, A> TxAncestors<'g, A> {
    fn populate_stack(&mut self, ancestor_depth: usize, tx: Transaction) {
        if ancestor_depth <= self.depth_limit && self.visited.insert(tx.txid()) {
            tx.input
                .iter()
                .map(|txin| txin.previous_output.txid)
                .for_each(|prev_txid| {
                    if let Some((TxNodeInternal::Whole(tx), _, _)) = self.graph.txs.get(&prev_txid)
                    {
                        self.stack.push((ancestor_depth, tx.clone()));
                    };
                });
        }
    }
}

impl<'g, A> Iterator for TxAncestors<'g, A> {
    type Item = (usize, Transaction);

    fn next(&mut self) -> Option<Self::Item> {
        let (ancestor_depth, tx) = self.stack.pop()?;
        self.populate_stack(ancestor_depth + 1, tx.clone());
        Some((ancestor_depth, tx))
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

    /// Creates a `TxDescendants` from multiple starting transactions that includes the starting
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
