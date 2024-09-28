//! Module for structures that store and traverse transactions.
//!
//! [`TxGraph`] contains transactions and indexes them so you can easily traverse the graph of
//! those transactions. `TxGraph` is *monotone* in that you can always insert a transaction -- it
//! does not care whether that transaction is in the current best chain or whether it conflicts with
//! any of the existing transactions or what order you insert the transactions. This means that you
//! can always combine two [`TxGraph`]s together, without resulting in inconsistencies. Furthermore,
//! there is currently no way to delete a transaction.
//!
//! Transactions can be either whole or partial (i.e., transactions for which we only know some
//! outputs, which we usually call "floating outputs"; these are usually inserted using the
//! [`insert_txout`] method.).
//!
//! The graph contains transactions in the form of [`TxNode`]s. Each node contains the txid, the
//! transaction (whole or partial), the blocks that it is anchored to (see the [`Anchor`]
//! documentation for more details), and the timestamp of the last time we saw the transaction as
//! unconfirmed.
//!
//! Conflicting transactions are allowed to coexist within a [`TxGraph`]. This is useful for
//! identifying and traversing conflicts and descendants of a given transaction. Some [`TxGraph`]
//! methods only consider transactions that are "canonical" (i.e., in the best chain or in mempool).
//! We decide which transactions are canonical based on the transaction's anchors and the
//! `last_seen` (as unconfirmed) timestamp; see the [`try_get_chain_position`] documentation for
//! more details.
//!
//! The [`ChangeSet`] reports changes made to a [`TxGraph`]; it can be used to either save to
//! persistent storage, or to be applied to another [`TxGraph`].
//!
//! Lastly, you can use [`TxAncestors`]/[`TxDescendants`] to traverse ancestors and descendants of
//! a given transaction, respectively.
//!
//! # Applying changes
//!
//! Methods that change the state of [`TxGraph`] will return [`ChangeSet`]s.
//! [`ChangeSet`]s can be applied back to a [`TxGraph`] or be used to inform persistent storage
//! of the changes to [`TxGraph`].
//!
//! # Generics
//!
//! Anchors are represented as generics within `TxGraph<A>`. To make use of all functionality of the
//! `TxGraph`, anchors (`A`) should implement [`Anchor`].
//!
//! Anchors are made generic so that different types of data can be stored with how a transaction is
//! *anchored* to a given block. An example of this is storing a merkle proof of the transaction to
//! the confirmation block - this can be done with a custom [`Anchor`] type. The minimal [`Anchor`]
//! type would just be a [`BlockId`] which just represents the height and hash of the block which
//! the transaction is contained in. Note that a transaction can be contained in multiple
//! conflicting blocks (by nature of the Bitcoin network).
//!
//! ```
//! # use bdk_chain::BlockId;
//! # use bdk_chain::tx_graph::TxGraph;
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! let mut tx_graph: TxGraph = TxGraph::default();
//!
//! // insert a transaction
//! let changeset = tx_graph.insert_tx(tx_a);
//!
//! // We can restore the state of the `tx_graph` by applying all
//! // the changesets obtained by mutating the original (the order doesn't matter).
//! let mut restored_tx_graph: TxGraph = TxGraph::default();
//! restored_tx_graph.apply_changeset(changeset);
//!
//! assert_eq!(tx_graph, restored_tx_graph);
//! ```
//!
//! A [`TxGraph`] can also be updated with another [`TxGraph`] which merges them together.
//!
//! ```
//! # use bdk_chain::{Merge, BlockId};
//! # use bdk_chain::tx_graph::{self, TxGraph};
//! # use bdk_chain::example_utils::*;
//! # use bitcoin::Transaction;
//! # use std::sync::Arc;
//! # let tx_a = tx_from_hex(RAW_TX_1);
//! # let tx_b = tx_from_hex(RAW_TX_2);
//! let mut graph: TxGraph = TxGraph::default();
//!
//! let mut update = tx_graph::TxUpdate::default();
//! update.txs.push(Arc::new(tx_a));
//! update.txs.push(Arc::new(tx_b));
//!
//! // apply the update graph
//! let changeset = graph.apply_update(update.clone());
//!
//! // if we apply it again, the resulting changeset will be empty
//! let changeset = graph.apply_update(update);
//! assert!(changeset.is_empty());
//! ```
//! [`try_get_chain_position`]: TxGraph::try_get_chain_position
//! [`insert_txout`]: TxGraph::insert_txout

use crate::collections::*;
use crate::BlockId;
use crate::{Anchor, Balance, ChainOracle, ChainPosition, FullTxOut, Merge};
use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
pub use bdk_core::TxUpdate;
use bitcoin::{Amount, OutPoint, ScriptBuf, SignedAmount, Transaction, TxOut, Txid};
use core::fmt::{self, Formatter};
use core::{
    convert::Infallible,
    ops::{Deref, RangeInclusive},
};

impl<A> From<TxGraph<A>> for TxUpdate<A> {
    fn from(graph: TxGraph<A>) -> Self {
        Self {
            txs: graph.full_txs().map(|tx_node| tx_node.tx).collect(),
            txouts: graph
                .floating_txouts()
                .map(|(op, txo)| (op, txo.clone()))
                .collect(),
            anchors: graph.anchors,
            seen_ats: graph.last_seen.into_iter().collect(),
        }
    }
}

impl<A: Ord + Clone> From<TxUpdate<A>> for TxGraph<A> {
    fn from(update: TxUpdate<A>) -> Self {
        let mut graph = TxGraph::<A>::default();
        let _ = graph.apply_update_at(update, None);
        graph
    }
}

/// A graph of transactions and spends.
///
/// See the [module-level documentation] for more.
///
/// [module-level documentation]: crate::tx_graph
#[derive(Clone, Debug, PartialEq)]
pub struct TxGraph<A = ()> {
    // all transactions that the graph is aware of in format: `(tx_node, tx_anchors)`
    txs: HashMap<Txid, (TxNodeInternal, BTreeSet<A>)>,
    spends: BTreeMap<OutPoint, HashSet<Txid>>,
    anchors: BTreeSet<(A, Txid)>,
    last_seen: HashMap<Txid, u64>,

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
            last_seen: Default::default(),
            empty_outspends: Default::default(),
        }
    }
}

/// A transaction node in the [`TxGraph`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TxNode<'a, T, A> {
    /// Txid of the transaction.
    pub txid: Txid,
    /// A partial or full representation of the transaction.
    pub tx: T,
    /// The blocks that the transaction is "anchored" in.
    pub anchors: &'a BTreeSet<A>,
    /// The last-seen unix timestamp of the transaction as unconfirmed.
    pub last_seen_unconfirmed: Option<u64>,
}

impl<'a, T, A> Deref for TxNode<'a, T, A> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

/// Internal representation of a transaction node of a [`TxGraph`].
///
/// This can either be a whole transaction, or a partial transaction (where we only have select
/// outputs).
#[derive(Clone, Debug, PartialEq)]
enum TxNodeInternal {
    Whole(Arc<Transaction>),
    Partial(BTreeMap<u32, TxOut>),
}

impl Default for TxNodeInternal {
    fn default() -> Self {
        Self::Partial(BTreeMap::new())
    }
}

/// A transaction that is included in the chain, or is still in mempool.
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
    NegativeFee(SignedAmount),
}

impl fmt::Display for CalculateFeeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CalculateFeeError::MissingTxOut(outpoints) => write!(
                f,
                "missing `TxOut` for one or more of the inputs of the tx: {:?}",
                outpoints
            ),
            CalculateFeeError::NegativeFee(fee) => write!(
                f,
                "transaction is invalid according to the graph and has negative fee: {}",
                fee.display_dynamic()
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CalculateFeeError {}

impl<A> TxGraph<A> {
    /// Iterate over all tx outputs known by [`TxGraph`].
    ///
    /// This includes txouts of both full transactions as well as floating transactions.
    pub fn all_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs.iter().flat_map(|(txid, (tx, _))| match tx {
            TxNodeInternal::Whole(tx) => tx
                .as_ref()
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
            .filter_map(|(txid, (tx_node, _))| match tx_node {
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
    pub fn full_txs(&self) -> impl Iterator<Item = TxNode<'_, Arc<Transaction>, A>> {
        self.txs
            .iter()
            .filter_map(|(&txid, (tx, anchors))| match tx {
                TxNodeInternal::Whole(tx) => Some(TxNode {
                    txid,
                    tx: tx.clone(),
                    anchors,
                    last_seen_unconfirmed: self.last_seen.get(&txid).copied(),
                }),
                TxNodeInternal::Partial(_) => None,
            })
    }

    /// Iterate over graph transactions with no anchors or last-seen.
    pub fn txs_with_no_anchor_or_last_seen(
        &self,
    ) -> impl Iterator<Item = TxNode<'_, Arc<Transaction>, A>> {
        self.full_txs().filter_map(|tx| {
            if tx.anchors.is_empty() && tx.last_seen_unconfirmed.is_none() {
                Some(tx)
            } else {
                None
            }
        })
    }

    /// Get a transaction by txid. This only returns `Some` for full transactions.
    ///
    /// Refer to [`get_txout`] for getting a specific [`TxOut`].
    ///
    /// [`get_txout`]: Self::get_txout
    pub fn get_tx(&self, txid: Txid) -> Option<Arc<Transaction>> {
        self.get_tx_node(txid).map(|n| n.tx)
    }

    /// Get a transaction node by txid. This only returns `Some` for full transactions.
    pub fn get_tx_node(&self, txid: Txid) -> Option<TxNode<'_, Arc<Transaction>, A>> {
        match &self.txs.get(&txid)? {
            (TxNodeInternal::Whole(tx), anchors) => Some(TxNode {
                txid,
                tx: tx.clone(),
                anchors,
                last_seen_unconfirmed: self.last_seen.get(&txid).copied(),
            }),
            _ => None,
        }
    }

    /// Obtains a single tx output (if any) at the specified outpoint.
    pub fn get_txout(&self, outpoint: OutPoint) -> Option<&TxOut> {
        match &self.txs.get(&outpoint.txid)?.0 {
            TxNodeInternal::Whole(tx) => tx.as_ref().output.get(outpoint.vout as usize),
            TxNodeInternal::Partial(txouts) => txouts.get(&outpoint.vout),
        }
    }

    /// Returns known outputs of a given `txid`.
    ///
    /// Returns a [`BTreeMap`] of vout to output of the provided `txid`.
    pub fn tx_outputs(&self, txid: Txid) -> Option<BTreeMap<u32, &TxOut>> {
        Some(match &self.txs.get(&txid)?.0 {
            TxNodeInternal::Whole(tx) => tx
                .as_ref()
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

    /// Calculates the fee of a given transaction. Returns [`Amount::ZERO`] if `tx` is a coinbase transaction.
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
    pub fn calculate_fee(&self, tx: &Transaction) -> Result<Amount, CalculateFeeError> {
        if tx.is_coinbase() {
            return Ok(Amount::ZERO);
        }

        let (inputs_sum, missing_outputs) = tx.input.iter().fold(
            (SignedAmount::ZERO, Vec::new()),
            |(mut sum, mut missing_outpoints), txin| match self.get_txout(txin.previous_output) {
                None => {
                    missing_outpoints.push(txin.previous_output);
                    (sum, missing_outpoints)
                }
                Some(txout) => {
                    sum += txout.value.to_signed().expect("valid `SignedAmount`");
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
            .map(|txout| txout.value.to_signed().expect("valid `SignedAmount`"))
            .sum::<SignedAmount>();

        let fee = inputs_sum - outputs_sum;
        fee.to_unsigned()
            .map_err(|_| CalculateFeeError::NegativeFee(fee))
    }

    /// The transactions spending from this output.
    ///
    /// [`TxGraph`] allows conflicting transactions within the graph. Obviously the transactions in
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
        let start = OutPoint::new(txid, 0);
        let end = OutPoint::new(txid, u32::MAX);
        self.spends
            .range(start..=end)
            .map(|(outpoint, spends)| (outpoint.vout, spends))
    }

    /// Verify the given transaction is able to spend its inputs.
    ///
    /// This method uses [`rust-bitcoinconsensus`][0] to verify a transaction, guaranteeing
    /// that if the method succeeds the transaction meets consensus criteria as defined in
    /// Bitcoin's `libbitcoinconsensus`.
    ///
    /// # Errors
    ///
    /// If the previous output isn't found for one or more `tx` inputs.
    ///
    /// If Bitcoin Script verification fails.
    ///
    /// [0]: https://docs.rs/bitcoinconsensus/latest/bitcoinconsensus/
    #[cfg(feature = "bitcoinconsensus")]
    #[cfg_attr(docsrs, doc(cfg(feature = "bitcoinconsensus")))]
    pub fn verify_tx(&self, tx: &Transaction) -> Result<(), VerifyTxError> {
        tx.verify(|op: &OutPoint| -> Option<TxOut> { self.get_txout(*op).cloned() })
            .map_err(VerifyTxError)
    }
}

/// Error returned by [`TxGraph::verify_tx`].
#[cfg(feature = "bitcoinconsensus")]
#[derive(Debug)]
pub struct VerifyTxError(pub bitcoin::transaction::TxVerifyError);

#[cfg(feature = "bitcoinconsensus")]
impl fmt::Display for VerifyTxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(feature = "std")]
#[cfg(feature = "bitcoinconsensus")]
impl std::error::Error for VerifyTxError {}

impl<A: Clone + Ord> TxGraph<A> {
    /// Creates an iterator that filters and maps ancestor transactions.
    ///
    /// The iterator starts with the ancestors of the supplied `tx` (ancestor transactions of `tx`
    /// are transactions spent by `tx`). The supplied transaction is excluded from the iterator.
    ///
    /// The supplied closure takes in two inputs `(depth, ancestor_tx)`:
    ///
    /// * `depth` is the distance between the starting `Transaction` and the `ancestor_tx`. I.e., if
    ///    the `Transaction` is spending an output of the `ancestor_tx` then `depth` will be 1.
    /// * `ancestor_tx` is the `Transaction`'s ancestor which we are considering to walk.
    ///
    /// The supplied closure returns an `Option<T>`, allowing the caller to map each `Transaction`
    /// it visits and decide whether to visit ancestors.
    pub fn walk_ancestors<'g, T, F, O>(&'g self, tx: T, walk_map: F) -> TxAncestors<'g, A, F>
    where
        T: Into<Arc<Transaction>>,
        F: FnMut(usize, Arc<Transaction>) -> Option<O> + 'g,
    {
        TxAncestors::new_exclude_root(self, tx, walk_map)
    }

    /// Creates an iterator that filters and maps descendants from the starting `txid`.
    ///
    /// The supplied closure takes in two inputs `(depth, descendant_txid)`:
    ///
    /// * `depth` is the distance between the starting `txid` and the `descendant_txid`. I.e., if the
    ///     descendant is spending an output of the starting `txid` then `depth` will be 1.
    /// * `descendant_txid` is the descendant's txid which we are considering to walk.
    ///
    /// The supplied closure returns an `Option<T>`, allowing the caller to map each node it visits
    /// and decide whether to visit descendants.
    pub fn walk_descendants<'g, F, O>(&'g self, txid: Txid, walk_map: F) -> TxDescendants<A, F>
    where
        F: FnMut(usize, Txid) -> Option<O> + 'g,
    {
        TxDescendants::new_exclude_root(self, txid, walk_map)
    }
}

impl<A> TxGraph<A> {
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
        let txids = self.direct_conflicts(tx).map(|(_, txid)| txid);
        TxDescendants::from_multiple_include_root(self, txids, walk_map)
    }

    /// Given a transaction, return an iterator of txids that directly conflict with the given
    /// transaction's inputs (spends). The conflicting txids are returned with the given
    /// transaction's vin (in which it conflicts).
    ///
    /// Note that this only returns directly conflicting txids and won't include:
    /// - descendants of conflicting transactions (which are technically also conflicting)
    /// - transactions conflicting with the given transaction's ancestors
    pub fn direct_conflicts<'g>(
        &'g self,
        tx: &'g Transaction,
    ) -> impl Iterator<Item = (usize, Txid)> + '_ {
        let txid = tx.compute_txid();
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
    /// Transform the [`TxGraph`] to have [`Anchor`]s of another type.
    ///
    /// This takes in a closure of signature `FnMut(A) -> A2` which is called for each [`Anchor`] to
    /// transform it.
    pub fn map_anchors<A2: Clone + Ord, F>(self, f: F) -> TxGraph<A2>
    where
        F: FnMut(A) -> A2,
    {
        let mut new_graph = TxGraph::<A2>::default();
        new_graph.apply_changeset(self.initial_changeset().map_anchors(f));
        new_graph
    }

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
        let mut changeset = ChangeSet::<A>::default();
        let (tx_node, _) = self.txs.entry(outpoint.txid).or_default();
        match tx_node {
            TxNodeInternal::Whole(_) => {
                // ignore this txout we have the full one already.
                // NOTE: You might think putting a debug_assert! here to check the output being
                // replaced was actually correct is a good idea but the tests have already been
                // written assuming this never panics.
            }
            TxNodeInternal::Partial(partial_tx) => {
                match partial_tx.insert(outpoint.vout, txout.clone()) {
                    Some(old_txout) => {
                        debug_assert_eq!(
                            txout, old_txout,
                            "txout of the same outpoint should never change"
                        );
                    }
                    None => {
                        changeset.txouts.insert(outpoint, txout);
                    }
                }
            }
        }
        changeset
    }

    /// Inserts the given transaction into [`TxGraph`].
    ///
    /// The [`ChangeSet`] returned will be empty if `tx` already exists.
    pub fn insert_tx<T: Into<Arc<Transaction>>>(&mut self, tx: T) -> ChangeSet<A> {
        let tx: Arc<Transaction> = tx.into();
        let txid = tx.compute_txid();
        let mut changeset = ChangeSet::<A>::default();

        let (tx_node, _) = self.txs.entry(txid).or_default();
        match tx_node {
            TxNodeInternal::Whole(existing_tx) => {
                debug_assert_eq!(
                    existing_tx.as_ref(),
                    tx.as_ref(),
                    "tx of same txid should never change"
                );
            }
            partial_tx => {
                for txin in &tx.input {
                    // this means the tx is coinbase so there is no previous output
                    if txin.previous_output.is_null() {
                        continue;
                    }
                    self.spends
                        .entry(txin.previous_output)
                        .or_default()
                        .insert(txid);
                }
                *partial_tx = TxNodeInternal::Whole(tx.clone());
                changeset.txs.insert(tx);
            }
        }

        changeset
    }

    /// Batch insert unconfirmed transactions.
    ///
    /// Items of `txs` are tuples containing the transaction and a *last seen* timestamp. The
    /// *last seen* communicates when the transaction is last seen in mempool which is used for
    /// conflict-resolution (refer to [`TxGraph::insert_seen_at`] for details).
    pub fn batch_insert_unconfirmed<T: Into<Arc<Transaction>>>(
        &mut self,
        txs: impl IntoIterator<Item = (T, u64)>,
    ) -> ChangeSet<A> {
        let mut changeset = ChangeSet::<A>::default();
        for (tx, seen_at) in txs {
            let tx: Arc<Transaction> = tx.into();
            changeset.merge(self.insert_seen_at(tx.compute_txid(), seen_at));
            changeset.merge(self.insert_tx(tx));
        }
        changeset
    }

    /// Inserts the given `anchor` into [`TxGraph`].
    ///
    /// The [`ChangeSet`] returned will be empty if graph already knows that `txid` exists in
    /// `anchor`.
    pub fn insert_anchor(&mut self, txid: Txid, anchor: A) -> ChangeSet<A> {
        let mut changeset = ChangeSet::<A>::default();
        if self.anchors.insert((anchor.clone(), txid)) {
            let (_tx_node, anchors) = self.txs.entry(txid).or_default();
            let _inserted = anchors.insert(anchor.clone());
            debug_assert!(
                _inserted,
                "anchors in `.anchors` and `.txs` should be consistent"
            );
            changeset.anchors.insert((anchor, txid));
        }
        changeset
    }

    /// Inserts the given `seen_at` for `txid` into [`TxGraph`].
    ///
    /// Note that [`TxGraph`] only keeps track of the latest `seen_at`.
    pub fn insert_seen_at(&mut self, txid: Txid, seen_at: u64) -> ChangeSet<A> {
        let mut changeset = ChangeSet::<A>::default();
        let last_seen = self.last_seen.entry(txid).or_default();
        if seen_at > *last_seen {
            *last_seen = seen_at;
            changeset.last_seen.insert(txid, seen_at);
        }
        changeset
    }

    /// Extends this graph with the given `update`.
    ///
    /// The returned [`ChangeSet`] is the set difference between `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    pub fn apply_update(&mut self, update: TxUpdate<A>) -> ChangeSet<A> {
        use std::time::*;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current time must be greater than epoch anchor");
        self.apply_update_at(update, Some(now.as_secs()))
    }

    /// Extends this graph with the given `update` alongside an optional `seen_at` timestamp.
    ///
    /// `seen_at` represents when the update is seen (in unix seconds). It is used to determine the
    /// `last_seen`s for all transactions in the update which have no corresponding anchor(s). The
    /// `last_seen` value is used internally to determine precedence of conflicting unconfirmed
    /// transactions (where the transaction with the lower `last_seen` value is omitted from the
    /// canonical history).
    ///
    /// Not setting a `seen_at` value means unconfirmed transactions introduced by this update will
    /// not be part of the canonical history of transactions.
    ///
    /// Use [`apply_update`](TxGraph::apply_update) to have the `seen_at` value automatically set
    /// to the current time.
    pub fn apply_update_at(&mut self, update: TxUpdate<A>, seen_at: Option<u64>) -> ChangeSet<A> {
        let mut changeset = ChangeSet::<A>::default();
        let mut unanchored_txs = HashSet::<Txid>::new();
        for tx in update.txs {
            if unanchored_txs.insert(tx.compute_txid()) {
                changeset.merge(self.insert_tx(tx));
            }
        }
        for (outpoint, txout) in update.txouts {
            changeset.merge(self.insert_txout(outpoint, txout));
        }
        for (anchor, txid) in update.anchors {
            unanchored_txs.remove(&txid);
            changeset.merge(self.insert_anchor(txid, anchor));
        }
        for (txid, seen_at) in update.seen_ats {
            changeset.merge(self.insert_seen_at(txid, seen_at));
        }
        if let Some(seen_at) = seen_at {
            for txid in unanchored_txs {
                changeset.merge(self.insert_seen_at(txid, seen_at));
            }
        }
        changeset
    }

    /// Determines the [`ChangeSet`] between `self` and an empty [`TxGraph`].
    pub fn initial_changeset(&self) -> ChangeSet<A> {
        ChangeSet {
            txs: self.full_txs().map(|tx_node| tx_node.tx).collect(),
            txouts: self
                .floating_txouts()
                .map(|(op, txout)| (op, txout.clone()))
                .collect(),
            anchors: self.anchors.clone(),
            last_seen: self.last_seen.iter().map(|(&k, &v)| (k, v)).collect(),
        }
    }

    /// Applies [`ChangeSet`] to [`TxGraph`].
    pub fn apply_changeset(&mut self, changeset: ChangeSet<A>) {
        for tx in changeset.txs {
            let _ = self.insert_tx(tx);
        }
        for (outpoint, txout) in changeset.txouts {
            let _ = self.insert_txout(outpoint, txout);
        }
        for (anchor, txid) in changeset.anchors {
            let _ = self.insert_anchor(txid, anchor);
        }
        for (txid, seen_at) in changeset.last_seen {
            let _ = self.insert_seen_at(txid, seen_at);
        }
    }
}

impl<A: Anchor> TxGraph<A> {
    /// Get the position of the transaction in `chain` with tip `chain_tip`.
    ///
    /// Chain data is fetched from `chain`, a [`ChainOracle`] implementation.
    ///
    /// This method returns `Ok(None)` if the transaction is not found in the chain, and no longer
    /// belongs in the mempool. The following factors are used to approximate whether an
    /// unconfirmed transaction exists in the mempool (not evicted):
    ///
    /// 1. Unconfirmed transactions that conflict with confirmed transactions are evicted.
    /// 2. Unconfirmed transactions that spend from transactions that are evicted, are also
    ///    evicted.
    /// 3. Given two conflicting unconfirmed transactions, the transaction with the lower
    ///    `last_seen_unconfirmed` parameter is evicted. A transaction's `last_seen_unconfirmed`
    ///    parameter is the max of all it's descendants' `last_seen_unconfirmed` parameters. If the
    ///    final `last_seen_unconfirmed`s are the same, the transaction with the lower `txid` (by
    ///    lexicographical order) is evicted.
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
        let (tx_node, anchors) = match self.txs.get(&txid) {
            Some(v) => v,
            None => return Ok(None),
        };

        for anchor in anchors {
            match chain.is_block_in_chain(anchor.anchor_block(), chain_tip)? {
                Some(true) => return Ok(Some(ChainPosition::Confirmed(anchor))),
                _ => continue,
            }
        }

        // If no anchors are in best chain and we don't have a last_seen, we can return
        // early because by definition the tx doesn't have a chain position.
        let last_seen = match self.last_seen.get(&txid) {
            Some(t) => *t,
            None => return Ok(None),
        };

        // The tx is not anchored to a block in the best chain, which means that it
        // might be in mempool, or it might have been dropped already.
        // Let's check conflicts to find out!
        let tx = match tx_node {
            TxNodeInternal::Whole(tx) => {
                // A coinbase tx that is not anchored in the best chain cannot be unconfirmed and
                // should always be filtered out.
                if tx.is_coinbase() {
                    return Ok(None);
                }
                tx.clone()
            }
            TxNodeInternal::Partial(_) => {
                // Partial transactions (outputs only) cannot have conflicts.
                return Ok(None);
            }
        };

        // We want to retrieve all the transactions that conflict with us, plus all the
        // transactions that conflict with our unconfirmed ancestors, since they conflict with us
        // as well.
        // We only traverse unconfirmed ancestors since conflicts of confirmed transactions
        // cannot be in the best chain.

        // First of all, we retrieve all our ancestors. Since we're using `new_include_root`, the
        // resulting array will also include `tx`
        let unconfirmed_ancestor_txs =
            TxAncestors::new_include_root(self, tx.clone(), |_, ancestor_tx: Arc<Transaction>| {
                let tx_node = self.get_tx_node(ancestor_tx.as_ref().compute_txid())?;
                // We're filtering the ancestors to keep only the unconfirmed ones (= no anchors in
                // the best chain)
                for block in tx_node.anchors {
                    match chain.is_block_in_chain(block.anchor_block(), chain_tip) {
                        Ok(Some(true)) => return None,
                        Err(e) => return Some(Err(e)),
                        _ => continue,
                    }
                }
                Some(Ok(tx_node))
            })
            .collect::<Result<Vec<_>, C::Error>>()?;

        // We determine our tx's last seen, which is the max between our last seen,
        // and our unconf descendants' last seen.
        let unconfirmed_descendants_txs = TxDescendants::new_include_root(
            self,
            tx.as_ref().compute_txid(),
            |_, descendant_txid: Txid| {
                let tx_node = self.get_tx_node(descendant_txid)?;
                // We're filtering the ancestors to keep only the unconfirmed ones (= no anchors in
                // the best chain)
                for block in tx_node.anchors {
                    match chain.is_block_in_chain(block.anchor_block(), chain_tip) {
                        Ok(Some(true)) => return None,
                        Err(e) => return Some(Err(e)),
                        _ => continue,
                    }
                }
                Some(Ok(tx_node))
            },
        )
        .collect::<Result<Vec<_>, C::Error>>()?;

        let tx_last_seen = unconfirmed_descendants_txs
            .iter()
            .max_by_key(|tx| tx.last_seen_unconfirmed)
            .map(|tx| tx.last_seen_unconfirmed)
            .expect("descendants always includes at least one transaction (the root tx");

        // Now we traverse our ancestors and consider all their conflicts
        for tx_node in unconfirmed_ancestor_txs {
            // We retrieve all the transactions conflicting with this specific ancestor
            let conflicting_txs =
                self.walk_conflicts(tx_node.tx.as_ref(), |_, txid| self.get_tx_node(txid));

            // If a conflicting tx is in the best chain, or has `last_seen` higher than this ancestor, then
            // this tx cannot exist in the best chain
            for conflicting_tx in conflicting_txs {
                for block in conflicting_tx.anchors {
                    if chain.is_block_in_chain(block.anchor_block(), chain_tip)? == Some(true) {
                        return Ok(None);
                    }
                }
                if conflicting_tx.last_seen_unconfirmed > tx_last_seen {
                    return Ok(None);
                }
                if conflicting_tx.last_seen_unconfirmed == Some(last_seen)
                    && conflicting_tx.as_ref().compute_txid() > tx.as_ref().compute_txid()
                {
                    // Conflicting tx has priority if txid of conflicting tx > txid of original tx
                    return Ok(None);
                }
            }
        }

        Ok(Some(ChainPosition::Unconfirmed(last_seen)))
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
    /// If the [`ChainOracle`] is infallible, [`list_canonical_txs`] can be used instead.
    ///
    /// [`list_canonical_txs`]: Self::list_canonical_txs
    pub fn try_list_canonical_txs<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<CanonicalTx<'a, Arc<Transaction>, A>, C::Error>> {
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
    /// This is the infallible version of [`try_list_canonical_txs`].
    ///
    /// [`try_list_canonical_txs`]: Self::try_list_canonical_txs
    pub fn list_canonical_txs<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = CanonicalTx<'a, Arc<Transaction>, A>> {
        self.try_list_canonical_txs(chain, chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }

    /// Get a filtered list of outputs from the given `outpoints` that are in `chain` with
    /// `chain_tip`.
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`OI`) for convenience. If `OI` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    ///
    /// Floating outputs (i.e., outputs for which we don't have the full transaction in the graph)
    /// are ignored.
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

                    let txout = match tx_node.tx.as_ref().output.get(op.vout as usize) {
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
                            is_on_coinbase: tx_node.tx.is_coinbase(),
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
        mut trust_predicate: impl FnMut(&OI, ScriptBuf) -> bool,
    ) -> Result<Balance, C::Error> {
        let mut immature = Amount::ZERO;
        let mut trusted_pending = Amount::ZERO;
        let mut untrusted_pending = Amount::ZERO;
        let mut confirmed = Amount::ZERO;

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
                    if trust_predicate(&spk_i, txout.txout.script_pubkey) {
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
        trust_predicate: impl FnMut(&OI, ScriptBuf) -> bool,
    ) -> Balance {
        self.try_balance(chain, chain_tip, outpoints, trust_predicate)
            .expect("oracle is infallible")
    }
}

/// The [`ChangeSet`] represents changes to a [`TxGraph`].
///
/// Since [`TxGraph`] is monotone, the "changeset" can only contain transactions to be added and
/// not removed.
///
/// Refer to [module-level documentation] for more.
///
/// [module-level documentation]: crate::tx_graph
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: Ord + serde::Deserialize<'de>",
        serialize = "A: Ord + serde::Serialize",
    ))
)]
#[must_use]
pub struct ChangeSet<A = ()> {
    /// Added transactions.
    pub txs: BTreeSet<Arc<Transaction>>,
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
    /// Iterates over all outpoints contained within [`ChangeSet`].
    pub fn txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs
            .iter()
            .flat_map(|tx| {
                tx.output
                    .iter()
                    .enumerate()
                    .map(move |(vout, txout)| (OutPoint::new(tx.compute_txid(), vout as _), txout))
            })
            .chain(self.txouts.iter().map(|(op, txout)| (*op, txout)))
    }

    /// Iterates over the heights of that the new transaction anchors in this changeset.
    ///
    /// This is useful if you want to find which heights you need to fetch data about in order to
    /// confirm or exclude these anchors.
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
}

impl<A: Ord> Merge for ChangeSet<A> {
    fn merge(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        self.txs.extend(other.txs);
        self.txouts.extend(other.txouts);
        self.anchors.extend(other.anchors);

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

impl<A: Ord> ChangeSet<A> {
    /// Transform the [`ChangeSet`] to have [`Anchor`]s of another type.
    ///
    /// This takes in a closure of signature `FnMut(A) -> A2` which is called for each [`Anchor`] to
    /// transform it.
    pub fn map_anchors<A2: Ord, F>(self, mut f: F) -> ChangeSet<A2>
    where
        F: FnMut(A) -> A2,
    {
        ChangeSet {
            txs: self.txs,
            txouts: self.txouts,
            anchors: BTreeSet::<(A2, Txid)>::from_iter(
                self.anchors.into_iter().map(|(a, txid)| (f(a), txid)),
            ),
            last_seen: self.last_seen,
        }
    }
}

impl<A> AsRef<TxGraph<A>> for TxGraph<A> {
    fn as_ref(&self) -> &TxGraph<A> {
        self
    }
}

/// An iterator that traverses ancestors of a given root transaction.
///
/// The iterator excludes partial transactions.
///
/// Returned by the [`walk_ancestors`] method of [`TxGraph`].
///
/// [`walk_ancestors`]: TxGraph::walk_ancestors
pub struct TxAncestors<'g, A, F> {
    graph: &'g TxGraph<A>,
    visited: HashSet<Txid>,
    queue: VecDeque<(usize, Arc<Transaction>)>,
    filter_map: F,
}

impl<'g, A, F> TxAncestors<'g, A, F> {
    /// Creates a `TxAncestors` that includes the starting `Transaction` when iterating.
    pub(crate) fn new_include_root(
        graph: &'g TxGraph<A>,
        tx: impl Into<Arc<Transaction>>,
        filter_map: F,
    ) -> Self {
        Self {
            graph,
            visited: Default::default(),
            queue: [(0, tx.into())].into(),
            filter_map,
        }
    }

    /// Creates a `TxAncestors` that excludes the starting `Transaction` when iterating.
    pub(crate) fn new_exclude_root(
        graph: &'g TxGraph<A>,
        tx: impl Into<Arc<Transaction>>,
        filter_map: F,
    ) -> Self {
        let mut ancestors = Self {
            graph,
            visited: Default::default(),
            queue: Default::default(),
            filter_map,
        };
        ancestors.populate_queue(1, tx.into());
        ancestors
    }

    /// Creates a `TxAncestors` from multiple starting `Transaction`s that includes the starting
    /// `Transaction`s when iterating.
    #[allow(unused)]
    pub(crate) fn from_multiple_include_root<I>(
        graph: &'g TxGraph<A>,
        txs: I,
        filter_map: F,
    ) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Arc<Transaction>>,
    {
        Self {
            graph,
            visited: Default::default(),
            queue: txs.into_iter().map(|tx| (0, tx.into())).collect(),
            filter_map,
        }
    }

    /// Creates a `TxAncestors` from multiple starting `Transaction`s that excludes the starting
    /// `Transaction`s when iterating.
    #[allow(unused)]
    pub(crate) fn from_multiple_exclude_root<I>(
        graph: &'g TxGraph<A>,
        txs: I,
        filter_map: F,
    ) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Arc<Transaction>>,
    {
        let mut ancestors = Self {
            graph,
            visited: Default::default(),
            queue: Default::default(),
            filter_map,
        };
        for tx in txs {
            ancestors.populate_queue(1, tx.into());
        }
        ancestors
    }

    fn populate_queue(&mut self, depth: usize, tx: Arc<Transaction>) {
        let ancestors = tx
            .input
            .iter()
            .map(|txin| txin.previous_output.txid)
            .filter(|&prev_txid| self.visited.insert(prev_txid))
            .filter_map(|prev_txid| self.graph.get_tx(prev_txid))
            .map(|tx| (depth, tx));
        self.queue.extend(ancestors);
    }
}

impl<'g, A, F, O> Iterator for TxAncestors<'g, A, F>
where
    F: FnMut(usize, Arc<Transaction>) -> Option<O>,
{
    type Item = O;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // we have exhausted all paths when queue is empty
            let (ancestor_depth, tx) = self.queue.pop_front()?;
            // ignore paths when user filters them out
            let item = match (self.filter_map)(ancestor_depth, tx.clone()) {
                Some(item) => item,
                None => continue,
            };
            self.populate_queue(ancestor_depth + 1, tx);
            return Some(item);
        }
    }
}

/// An iterator that traverses transaction descendants.
///
/// Returned by the [`walk_descendants`] method of [`TxGraph`].
///
/// [`walk_descendants`]: TxGraph::walk_descendants
pub struct TxDescendants<'g, A, F> {
    graph: &'g TxGraph<A>,
    visited: HashSet<Txid>,
    queue: VecDeque<(usize, Txid)>,
    filter_map: F,
}

impl<'g, A, F> TxDescendants<'g, A, F> {
    /// Creates a `TxDescendants` that includes the starting `txid` when iterating.
    #[allow(unused)]
    pub(crate) fn new_include_root(graph: &'g TxGraph<A>, txid: Txid, filter_map: F) -> Self {
        Self {
            graph,
            visited: Default::default(),
            queue: [(0, txid)].into(),
            filter_map,
        }
    }

    /// Creates a `TxDescendants` that excludes the starting `txid` when iterating.
    pub(crate) fn new_exclude_root(graph: &'g TxGraph<A>, txid: Txid, filter_map: F) -> Self {
        let mut descendants = Self {
            graph,
            visited: Default::default(),
            queue: Default::default(),
            filter_map,
        };
        descendants.populate_queue(1, txid);
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
            queue: txids.into_iter().map(|txid| (0, txid)).collect(),
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
            queue: Default::default(),
            filter_map,
        };
        for txid in txids {
            descendants.populate_queue(1, txid);
        }
        descendants
    }
}

impl<'g, A, F> TxDescendants<'g, A, F> {
    fn populate_queue(&mut self, depth: usize, txid: Txid) {
        let spend_paths = self
            .graph
            .spends
            .range(tx_outpoint_range(txid))
            .flat_map(|(_, spends)| spends)
            .map(|&txid| (depth, txid));
        self.queue.extend(spend_paths);
    }
}

impl<'g, A, F, O> Iterator for TxDescendants<'g, A, F>
where
    F: FnMut(usize, Txid) -> Option<O>,
{
    type Item = O;

    fn next(&mut self) -> Option<Self::Item> {
        let (op_spends, txid, item) = loop {
            // we have exhausted all paths when queue is empty
            let (op_spends, txid) = self.queue.pop_front()?;
            // we do not want to visit the same transaction twice
            if self.visited.insert(txid) {
                // ignore paths when user filters them out
                if let Some(item) = (self.filter_map)(op_spends, txid) {
                    break (op_spends, txid, item);
                }
            }
        };

        self.populate_queue(op_spends + 1, txid);
        Some(item)
    }
}

fn tx_outpoint_range(txid: Txid) -> RangeInclusive<OutPoint> {
    OutPoint::new(txid, u32::MIN)..=OutPoint::new(txid, u32::MAX)
}
