//! The [`TxGraph`] stores and indexes transactions so you can easily traverse the resulting graph. The
//! nodes are transactions and the edges are spends. `TxGraph` is *monotone* in that you can always
//! insert a transaction -- it does not care whether that transaction is in the current best chain
//! or whether it conflicts with any of the existing transactions or what order you insert the
//! transactions. This means that you can always combine two [`TxGraph`]s together, without
//! resulting in inconsistencies. Furthermore, there is currently no way to delete a transaction.
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
//! `last_seen` (as unconfirmed) timestamp.
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
//! ## Anchors
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
//! ## Indexer
//!
//! You can add your own [`Indexer`] to the transaction graph which will be notified whenever new
//! transaction data is attached to the graph. Usually this is instantiated with indexers like
//! [`KeychainTxOutIndex`] to keep track of transaction outputs owned by an output descriptor.
//!
//! # Examples
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
//! let mut tx_graph: TxGraph = TxGraph::default();
//!
//! let mut update = tx_graph::TxUpdate::default();
//! update.txs.push(Arc::new(tx_a));
//! update.txs.push(Arc::new(tx_b));
//!
//! // apply the update graph
//! let changeset = tx_graph.apply_update(update.clone());
//!
//! // if we apply it again, the resulting changeset will be empty
//! let changeset = tx_graph.apply_update(update);
//! assert!(changeset.is_empty());
//! ```
//! [`insert_txout`]: TxGraph::insert_txout
//! [`KeychainTxOutIndex`]: crate::indexer::keychain_txout::KeychainTxOutIndex

use crate::collections::*;
use crate::BlockId;
use crate::CanonicalIter;
use crate::CanonicalReason;
use crate::Indexer;
use crate::ObservedIn;
use crate::TxPosInBlock;
use crate::{Anchor, Balance, ChainOracle, ChainPosition, FullTxOut, Merge};
use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bdk_core::ConfirmationBlockTime;
pub use bdk_core::TxUpdate;
use bitcoin::{Amount, Block, OutPoint, ScriptBuf, SignedAmount, Transaction, TxOut, Txid};
use core::fmt::{self, Formatter};
use core::{
    convert::Infallible,
    ops::{Deref, RangeInclusive},
};

impl<A: Ord, X> From<TxGraph<A, X>> for TxUpdate<A> {
    fn from(graph: TxGraph<A, X>) -> Self {
        let mut tx_update = TxUpdate::default();
        tx_update.txs = graph.full_txs().map(|tx_node| tx_node.tx).collect();
        tx_update.txouts = graph
            .floating_txouts()
            .map(|(op, txo)| (op, txo.clone()))
            .collect();
        tx_update.anchors = graph
            .anchors
            .into_iter()
            .flat_map(|(txid, anchors)| anchors.into_iter().map(move |a| (a, txid)))
            .collect();
        tx_update.seen_ats = graph.last_seen.into_iter().collect();
        tx_update
    }
}

impl<A: Anchor> From<TxUpdate<A>> for TxGraph<A> {
    fn from(update: TxUpdate<A>) -> Self {
        let mut graph = TxGraph::<A>::default();
        let _ = graph.apply_update(update);
        graph
    }
}

/// A graph of transactions and spends.
///
/// See the [module-level documentation] for more.
///
/// [module-level documentation]: crate::tx_graph
#[derive(Clone, Debug, PartialEq)]
pub struct TxGraph<A = ConfirmationBlockTime, X = ()> {
    txs: HashMap<Txid, TxNodeInternal>,
    spends: BTreeMap<OutPoint, HashSet<Txid>>,
    anchors: HashMap<Txid, BTreeSet<A>>,
    last_seen: HashMap<Txid, u64>,

    txs_by_highest_conf_heights: BTreeSet<(u32, Txid)>,
    txs_by_last_seen: BTreeSet<(u64, Txid)>,

    // The following fields exist so that methods can return references to empty sets.
    // FIXME: This can be removed once `HashSet::new` and `BTreeSet::new` are const fns.
    empty_outspends: HashSet<Txid>,
    empty_anchors: BTreeSet<A>,

    /// Indexer. This is called to index all transactions and outpoints added to the transaction
    /// graph.
    pub index: X,
}

impl<A, X> Default for TxGraph<A, X>
where
    X: Default,
{
    fn default() -> Self {
        Self {
            txs: Default::default(),
            spends: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
            txs_by_highest_conf_heights: Default::default(),
            txs_by_last_seen: Default::default(),
            empty_outspends: Default::default(),
            empty_anchors: Default::default(),
            index: Default::default(),
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

impl<T, A> Deref for TxNode<'_, T, A> {
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

impl TxNodeInternal {
    /// Iterate over outpoint and txouts of a partial tx node. The caller is
    /// responsible for matching the given `partial` with the correct `txid`.
    fn iter_txouts(
        txid: Txid,
        partial: &BTreeMap<u32, TxOut>,
    ) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        partial
            .iter()
            .map(move |(&vout, txout)| (OutPoint { txid, vout }, txout))
    }
}

impl Default for TxNodeInternal {
    fn default() -> Self {
        Self::Partial(BTreeMap::new())
    }
}

/// A transaction that is deemed to be part of the canonical history.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalTx<'a, T, A> {
    /// How the transaction is observed in the canonical chain (confirmed or unconfirmed).
    pub chain_position: ChainPosition<A>,
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

impl<A, X> TxGraph<A, X> {
    /// Iterate over all tx outputs known by [`TxGraph`].
    ///
    /// This includes txouts of both full transactions as well as floating transactions.
    pub fn all_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs.iter().flat_map(|(&txid, tx)| match tx {
            TxNodeInternal::Whole(tx) => tx
                .as_ref()
                .output
                .iter()
                .enumerate()
                .map(|(vout, txout)| (OutPoint::new(txid, vout as _), txout))
                .collect::<Vec<_>>(),
            TxNodeInternal::Partial(partial) => {
                TxNodeInternal::iter_txouts(txid, partial).collect::<Vec<_>>()
            }
        })
    }

    /// Iterate over floating txouts known by [`TxGraph`].
    ///
    /// Floating txouts are txouts that do not have the residing full transaction contained in the
    /// graph.
    pub fn floating_txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs
            .iter()
            .filter_map(|(&txid, tx_node)| match tx_node {
                TxNodeInternal::Whole(_) => None,
                TxNodeInternal::Partial(partial) => {
                    Some(TxNodeInternal::iter_txouts(txid, partial))
                }
            })
            .flatten()
    }

    /// Iterate over all full transactions in the graph.
    pub fn full_txs(&self) -> impl Iterator<Item = TxNode<'_, Arc<Transaction>, A>> {
        self.txs.iter().filter_map(|(&txid, tx)| match tx {
            TxNodeInternal::Whole(tx) => Some(TxNode {
                txid,
                tx: tx.clone(),
                anchors: self.get_anchors(txid),
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
            TxNodeInternal::Whole(tx) => Some(TxNode {
                txid,
                tx: tx.clone(),
                anchors: self.get_anchors(txid),
                last_seen_unconfirmed: self.last_seen.get(&txid).copied(),
            }),
            _ => None,
        }
    }

    /// Obtains a single tx output (if any) at the specified outpoint.
    pub fn get_txout(&self, outpoint: OutPoint) -> Option<&TxOut> {
        match &self.txs.get(&outpoint.txid)? {
            TxNodeInternal::Whole(tx) => tx.as_ref().output.get(outpoint.vout as usize),
            TxNodeInternal::Partial(txouts) => txouts.get(&outpoint.vout),
        }
    }

    /// Get the anchors associated with a transaction.
    pub fn get_anchors(&self, txid: Txid) -> &BTreeSet<A> {
        self.anchors.get(&txid).unwrap_or(&self.empty_anchors)
    }

    /// Returns known outputs of a given `txid`.
    ///
    /// Returns a [`BTreeMap`] of vout to output of the provided `txid`.
    pub fn tx_outputs(&self, txid: Txid) -> Option<BTreeMap<u32, &TxOut>> {
        Some(match &self.txs.get(&txid)? {
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
}

impl<A: Clone + Ord, X> TxGraph<A, X> {
    /// Return the anchors as elements of a reverse map
    fn rev_anchors(&self) -> BTreeSet<(A, Txid)> {
        self.anchors
            .iter()
            .flat_map(|(&txid, anchors)| anchors.iter().cloned().map(move |a| (a, txid)))
            .collect()
    }

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
    pub fn walk_ancestors<'g, T, F, O>(&'g self, tx: T, walk_map: F) -> TxAncestors<'g, A, X, F, O>
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
    pub fn walk_descendants<'g, F, O>(
        &'g self,
        txid: Txid,
        walk_map: F,
    ) -> TxDescendants<'g, A, X, F, O>
    where
        F: FnMut(usize, Txid) -> Option<O> + 'g,
    {
        TxDescendants::new_exclude_root(self, txid, walk_map)
    }
}

impl<A, X> TxGraph<A, X> {
    /// Creates an iterator that both filters and maps conflicting transactions (this includes
    /// descendants of directly-conflicting transactions, which are also considered conflicts).
    ///
    /// Refer to [`Self::walk_descendants`] for `walk_map` usage.
    pub fn walk_conflicts<'g, F, O>(
        &'g self,
        tx: &'g Transaction,
        walk_map: F,
    ) -> TxDescendants<'g, A, X, F, O>
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
    ) -> impl Iterator<Item = (usize, Txid)> + 'g {
        let txid = tx.compute_txid();
        tx.input
            .iter()
            .enumerate()
            .filter_map(move |(vin, txin)| self.spends.get(&txin.previous_output).zip(Some(vin)))
            .flat_map(|(spends, vin)| core::iter::repeat(vin).zip(spends.iter().cloned()))
            .filter(move |(_, conflicting_txid)| *conflicting_txid != txid)
    }

    /// Get all transaction anchors known by [`TxGraph`].
    pub fn all_anchors(&self) -> &HashMap<Txid, BTreeSet<A>> {
        &self.anchors
    }

    /// Whether the graph has any transactions or outputs in it.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }
}

impl<A: Anchor, X: Indexer> TxGraph<A, X> {
    /// Transform the [`TxGraph`] to have [`Anchor`]s of another type.
    ///
    /// This takes in a closure of signature `FnMut(A) -> A2` which is called for each [`Anchor`] to
    /// transform it.
    pub fn map_anchors<A2: Anchor, F>(self, mut f: F) -> TxGraph<A2, X>
    where
        F: FnMut(A) -> A2,
    {
        let new_anchors = self.rev_anchors().into_iter().map(|(a, txid)| (f(a), txid));

        let mut new_graph: TxGraph<A2, X> = TxGraph {
            txs: self.txs,
            spends: self.spends,
            anchors: Default::default(),
            last_seen: self.last_seen,
            txs_by_highest_conf_heights: self.txs_by_highest_conf_heights,
            txs_by_last_seen: self.txs_by_last_seen,
            empty_anchors: Default::default(),
            empty_outspends: Default::default(),
            index: self.index,
        };

        for (new_anchor, txid) in new_anchors {
            let _ = new_graph.insert_anchor(txid, new_anchor);
        }

        new_graph
    }

    /// Construct a new [`TxGraph`] from [`Indexer`].
    pub fn new(indexer: X) -> Self
    where
        X: Default,
    {
        Self {
            index: indexer,
            ..Default::default()
        }
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
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) -> ChangeSet<A, X::ChangeSet> {
        let mut changeset = ChangeSet::default();
        let tx_node = self.txs.entry(outpoint.txid).or_default();
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
                        changeset.tx_data.txouts.insert(outpoint, txout);
                    }
                }
            }
        }
        self.index_changeset(&mut changeset);
        changeset
    }

    /// Inserts the given transaction into [`TxGraph`].
    ///
    /// The [`ChangeSet`] returned will be empty if `tx` already exists.
    pub fn insert_tx<T: Into<Arc<Transaction>>>(&mut self, tx: T) -> ChangeSet<A, X::ChangeSet> {
        let tx: Arc<Transaction> = tx.into();
        let txid = tx.compute_txid();
        let mut changeset = ChangeSet::default();

        let tx_node = self.txs.entry(txid).or_default();
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
                changeset.tx_data.txs.insert(tx);
            }
        }

        self.index_changeset(&mut changeset);

        changeset
    }

    /// Inserts a batch of transactions at once and returns the [`ChangeSet`].
    pub fn batch_insert_tx<T: Into<Arc<Transaction>>>(
        &mut self,
        txs: impl IntoIterator<Item = T>,
    ) -> ChangeSet<A, X::ChangeSet> {
        let mut changeset = ChangeSet::default();
        for tx in txs {
            changeset.merge(self.insert_tx(tx));
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
    ) -> ChangeSet<A, X::ChangeSet> {
        let mut changeset = ChangeSet::default();
        for (tx, seen_at) in txs {
            let tx: Arc<Transaction> = tx.into();
            changeset.merge(self.insert_seen_at(tx.compute_txid(), seen_at));
            changeset.merge(self.insert_tx(tx));
        }
        changeset
    }

    /// Batch insert transactions, filtering out those that are irrelevant.
    ///
    /// Relevancy is determined by the [`Indexer::is_tx_relevant`] implementation of `I`. Irrelevant
    /// transactions in `txs` will be ignored. `txs` do not need to be in topological order.
    ///
    /// The second item in the input tuple is a list of *anchors* for the transaction. This is
    /// usually a single item or none at all. Therefore using an `Option<A>` as the type is the most
    /// ergonomic approach.
    pub fn batch_insert_relevant<T: Into<Arc<Transaction>>>(
        &mut self,
        txs: impl IntoIterator<Item = (T, impl IntoIterator<Item = A>)>,
    ) -> ChangeSet<A, X::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let txs = txs
            .into_iter()
            .map(|(tx, anchors)| (<T as Into<Arc<Transaction>>>::into(tx), anchors))
            .collect::<Vec<_>>();

        let mut changeset = ChangeSet::default();

        for (tx, _) in &txs {
            changeset.merge(ChangeSet {
                indexer: self.index.index_tx(tx),
                ..Default::default()
            });
        }

        for (tx, anchors) in txs {
            if self.index.is_tx_relevant(&tx) {
                let txid = tx.compute_txid();
                changeset.merge(self.insert_tx(tx.clone()));
                for anchor in anchors {
                    changeset.merge(self.insert_anchor(txid, anchor));
                }
            }
        }

        changeset
    }

    /// Batch insert unconfirmed transactions, filtering out those that are irrelevant.
    ///
    /// Relevancy is determined by the internal [`Indexer::is_tx_relevant`] implementation of `I`.
    /// Irrelevant transactions in `txs` will be ignored.
    ///
    /// Items of `txs` are tuples containing the transaction and a *last seen* timestamp. The
    /// *last seen* communicates when the transaction is last seen in the mempool which is used for
    /// conflict-resolution in [`TxGraph`] (refer to [`TxGraph::insert_seen_at`] for details).
    pub fn batch_insert_relevant_unconfirmed<T: Into<Arc<Transaction>>>(
        &mut self,
        unconfirmed_txs: impl IntoIterator<Item = (T, u64)>,
    ) -> ChangeSet<A, X::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let mut txs = unconfirmed_txs
            .into_iter()
            .map(|(tx, last_seen)| (<T as Into<Arc<Transaction>>>::into(tx), last_seen))
            .collect::<Vec<_>>();

        let mut changeset = ChangeSet::<A, X::ChangeSet>::default();
        for (tx, _) in &txs {
            changeset.indexer.merge(self.index.index_tx(tx));
        }

        txs.retain(|(tx, _)| self.index.is_tx_relevant(tx));

        changeset.merge(self.batch_insert_unconfirmed(txs));

        changeset
    }

    /// Inserts the given `anchor` into [`TxGraph`].
    ///
    /// The [`ChangeSet`] returned will be empty if graph already knows that `txid` exists in
    /// `anchor`.
    pub fn insert_anchor(&mut self, txid: Txid, anchor: A) -> ChangeSet<A, X::ChangeSet> {
        // These two variables are used to determine how to modify the `txid`'s entry in
        // `txs_by_highest_conf_heights`.
        // We want to remove `(old_top_h?, txid)` and insert `(new_top_h?, txid)`.
        let mut old_top_h = None;
        let mut new_top_h = anchor.confirmation_height_upper_bound();

        let is_changed = match self.anchors.entry(txid) {
            hash_map::Entry::Occupied(mut e) => {
                old_top_h = e
                    .get()
                    .iter()
                    .last()
                    .map(Anchor::confirmation_height_upper_bound);
                if let Some(old_top_h) = old_top_h {
                    if old_top_h > new_top_h {
                        new_top_h = old_top_h;
                    }
                }
                let is_changed = e.get_mut().insert(anchor.clone());
                is_changed
            }
            hash_map::Entry::Vacant(e) => {
                e.insert(core::iter::once(anchor.clone()).collect());
                true
            }
        };

        let mut changeset = ChangeSet::default();
        if is_changed {
            let new_top_is_changed = match old_top_h {
                None => true,
                Some(old_top_h) if old_top_h != new_top_h => true,
                _ => false,
            };
            if new_top_is_changed {
                if let Some(prev_top_h) = old_top_h {
                    self.txs_by_highest_conf_heights.remove(&(prev_top_h, txid));
                }
                self.txs_by_highest_conf_heights.insert((new_top_h, txid));
            }
            changeset.tx_data.anchors.insert((anchor, txid));
        }
        changeset
    }

    /// Inserts the given `seen_at` for `txid` into [`TxGraph`].
    ///
    /// Note that [`TxGraph`] only keeps track of the latest `seen_at`.
    pub fn insert_seen_at(&mut self, txid: Txid, seen_at: u64) -> ChangeSet<A, X::ChangeSet> {
        let mut old_last_seen = None;
        let is_changed = match self.last_seen.entry(txid) {
            hash_map::Entry::Occupied(mut e) => {
                let last_seen = e.get_mut();
                old_last_seen = Some(*last_seen);
                let change = *last_seen < seen_at;
                if change {
                    *last_seen = seen_at;
                }
                change
            }
            hash_map::Entry::Vacant(e) => {
                e.insert(seen_at);
                true
            }
        };

        let mut changeset = ChangeSet::default();
        if is_changed {
            if let Some(old_last_seen) = old_last_seen {
                self.txs_by_last_seen.remove(&(old_last_seen, txid));
            }
            self.txs_by_last_seen.insert((seen_at, txid));
            changeset.tx_data.last_seen.insert(txid, seen_at);
        }
        changeset
    }

    /// Extends this graph with the given `update`.
    ///
    /// The returned [`ChangeSet`] is the set difference between `update` and `self` (transactions that
    /// exist in `update` but not in `self`).
    pub fn apply_update(&mut self, update: TxUpdate<A>) -> ChangeSet<A, X::ChangeSet> {
        let mut changeset = ChangeSet::<A, X::ChangeSet>::default();
        for tx in update.txs {
            changeset.merge(self.insert_tx(tx));
        }
        for (outpoint, txout) in update.txouts {
            changeset.merge(self.insert_txout(outpoint, txout));
        }
        for (anchor, txid) in update.anchors {
            changeset.merge(self.insert_anchor(txid, anchor));
        }
        for (txid, seen_at) in update.seen_ats {
            changeset.merge(self.insert_seen_at(txid, seen_at));
        }
        self.index_changeset(&mut changeset);
        changeset
    }

    /// Changes the [`Indexer`] for a `TxGraph`. **This doesn't re-index the transactions in the
    /// graph**. To do that that call [`reindex`].
    ///
    /// Returns the new `TxGraph` and the old indexer.
    ///
    /// [`reindex`]: Self::reindex
    pub fn swap_indexer<X2>(self, indexer: X2) -> (TxGraph<A, X2>, X) {
        (
            TxGraph {
                txs: self.txs,
                spends: self.spends,
                anchors: self.anchors,
                last_seen: self.last_seen,
                txs_by_highest_conf_heights: self.txs_by_highest_conf_heights,
                txs_by_last_seen: self.txs_by_last_seen,
                empty_outspends: self.empty_outspends,
                empty_anchors: self.empty_anchors,
                index: indexer,
            },
            self.index,
        )
    }

    /// Reindexes the transaction graph by calling the [`Indexer`] on every transaction.
    ///
    /// The returned changeset will only be non-empty in the `indexer` field.
    pub fn reindex(&mut self) -> ChangeSet<A, X::ChangeSet> {
        let mut changeset = ChangeSet::<A, X::ChangeSet>::default();
        for (&txid, node) in &self.txs {
            match node {
                TxNodeInternal::Whole(tx) => changeset.merge(ChangeSet {
                    indexer: self.index.index_tx(tx.as_ref()),
                    ..Default::default()
                }),
                TxNodeInternal::Partial(txouts) => {
                    for (op, txout) in TxNodeInternal::iter_txouts(txid, txouts) {
                        changeset.merge(ChangeSet {
                            indexer: self.index.index_txout(op, txout),
                            ..Default::default()
                        });
                    }
                }
            };
        }
        changeset
    }

    /// Determines the [`ChangeSet`] between `self` and an empty [`TxGraph`].
    pub fn initial_changeset(&self) -> ChangeSet<A, X::ChangeSet> {
        let tx_data = TxChangeSet {
            txs: self.full_txs().map(|tx_node| tx_node.tx).collect(),
            txouts: self
                .floating_txouts()
                .map(|(op, txout)| (op, txout.clone()))
                .collect(),
            anchors: self.rev_anchors(),
            last_seen: self.last_seen.clone().into_iter().collect(),
        };
        ChangeSet {
            tx_data,
            indexer: self.index.initial_changeset(),
        }
    }

    /// Indexes the txs and txouts of `changeset` and merges the resulting changes.
    fn index_changeset(&mut self, changeset: &mut ChangeSet<A, X::ChangeSet>) {
        let indexer = &mut changeset.indexer;
        for tx in &changeset.tx_data.txs {
            indexer.merge(self.index.index_tx(tx));
        }
        for (&outpoint, txout) in &changeset.tx_data.txouts {
            indexer.merge(self.index.index_txout(outpoint, txout));
        }
    }

    /// Applies [`ChangeSet`] to [`TxGraph`].
    pub fn apply_changeset(&mut self, changeset: ChangeSet<A, X::ChangeSet>) {
        // It's possible that the changeset contains some state for the indexer to help it index the
        // transactions so we do that first.
        self.index.apply_changeset(changeset.indexer);

        let tx_changeset = changeset.tx_data;
        for tx in tx_changeset.txs {
            let _ = self.insert_tx(tx);
        }
        for (outpoint, txout) in tx_changeset.txouts {
            let _ = self.insert_txout(outpoint, txout);
        }
        for (anchor, txid) in tx_changeset.anchors {
            let _ = self.insert_anchor(txid, anchor);
        }
        for (txid, seen_at) in tx_changeset.last_seen {
            let _ = self.insert_seen_at(txid, seen_at);
        }
    }
}

impl<A: Anchor, X> TxGraph<A, X> {
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
        self.canonical_iter(chain, chain_tip).flat_map(move |res| {
            res.map(|(txid, _, canonical_reason)| {
                let tx_node = self.get_tx_node(txid).expect("must contain tx");
                let chain_position = match canonical_reason {
                    CanonicalReason::Anchor { anchor, descendant } => match descendant {
                        Some(_) => {
                            let direct_anchor = tx_node
                                .anchors
                                .iter()
                                .find_map(|a| -> Option<Result<A, C::Error>> {
                                    match chain.is_block_in_chain(a.anchor_block(), chain_tip) {
                                        Ok(Some(true)) => Some(Ok(a.clone())),
                                        Ok(Some(false)) | Ok(None) => None,
                                        Err(err) => Some(Err(err)),
                                    }
                                })
                                .transpose()?;
                            match direct_anchor {
                                Some(anchor) => ChainPosition::Confirmed {
                                    anchor,
                                    transitively: None,
                                },
                                None => ChainPosition::Confirmed {
                                    anchor,
                                    transitively: descendant,
                                },
                            }
                        }
                        None => ChainPosition::Confirmed {
                            anchor,
                            transitively: None,
                        },
                    },
                    CanonicalReason::ObservedIn { observed_in, .. } => match observed_in {
                        ObservedIn::Mempool(last_seen) => ChainPosition::Unconfirmed {
                            last_seen: Some(last_seen),
                        },
                        ObservedIn::Block(_) => ChainPosition::Unconfirmed { last_seen: None },
                    },
                };
                Ok(CanonicalTx {
                    chain_position,
                    tx_node,
                })
            })
        })
    }

    /// List graph transactions that are in `chain` with `chain_tip`.
    ///
    /// This is the infallible version of [`try_list_canonical_txs`].
    ///
    /// [`try_list_canonical_txs`]: Self::try_list_canonical_txs
    pub fn list_canonical_txs<'a, C: ChainOracle<Error = Infallible> + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = CanonicalTx<'a, Arc<Transaction>, A>> {
        self.try_list_canonical_txs(chain, chain_tip)
            .map(|res| res.expect("infallible"))
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
    ) -> Result<impl Iterator<Item = (OI, FullTxOut<A>)> + 'a, C::Error> {
        let mut canon_txs = HashMap::<Txid, CanonicalTx<Arc<Transaction>, A>>::new();
        let mut canon_spends = HashMap::<OutPoint, Txid>::new();
        for r in self.try_list_canonical_txs(chain, chain_tip) {
            let canonical_tx = r?;
            let txid = canonical_tx.tx_node.txid;

            if !canonical_tx.tx_node.tx.is_coinbase() {
                for txin in &canonical_tx.tx_node.tx.input {
                    let _res = canon_spends.insert(txin.previous_output, txid);
                    assert!(
                        _res.is_none(),
                        "tried to replace {:?} with {:?}",
                        _res,
                        txid
                    );
                }
            }
            canon_txs.insert(txid, canonical_tx);
        }
        Ok(outpoints.into_iter().filter_map(move |(spk_i, outpoint)| {
            let canon_tx = canon_txs.get(&outpoint.txid)?;
            let txout = canon_tx
                .tx_node
                .tx
                .output
                .get(outpoint.vout as usize)
                .cloned()?;
            let chain_position = canon_tx.chain_position.clone();
            let spent_by = canon_spends.get(&outpoint).map(|spend_txid| {
                let spend_tx = canon_txs
                    .get(spend_txid)
                    .cloned()
                    .expect("must be canonical");
                (spend_tx.chain_position, *spend_txid)
            });
            let is_on_coinbase = canon_tx.tx_node.is_coinbase();
            Some((
                spk_i,
                FullTxOut {
                    outpoint,
                    txout,
                    chain_position,
                    spent_by,
                    is_on_coinbase,
                },
            ))
        }))
    }

    /// List txids by descending anchor height order.
    ///
    /// If multiple anchors exist for a txid, the highest anchor height will be used. Transactions
    /// without anchors are excluded.
    pub fn txids_by_descending_anchor_height(
        &self,
    ) -> impl ExactSizeIterator<Item = (u32, Txid)> + '_ {
        self.txs_by_highest_conf_heights.iter().copied().rev()
    }

    /// List txids by descending last-seen order.
    ///
    /// Transactions without last-seens are excluded.
    pub fn txids_by_descending_last_seen(&self) -> impl ExactSizeIterator<Item = (u64, Txid)> + '_ {
        self.txs_by_last_seen.iter().copied().rev()
    }

    /// Returns a [`CanonicalIter`].
    pub fn canonical_iter<'a, C: ChainOracle>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> CanonicalIter<'a, A, X, C> {
        CanonicalIter::new(self, chain, chain_tip)
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
            .expect("oracle is infallible")
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
    ) -> Result<impl Iterator<Item = (OI, FullTxOut<A>)> + 'a, C::Error> {
        Ok(self
            .try_filter_chain_txouts(chain, chain_tip, outpoints)?
            .filter(|(_, full_txo)| full_txo.spent_by.is_none()))
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
            .expect("oracle is infallible")
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

        for (spk_i, txout) in self.try_filter_chain_unspents(chain, chain_tip, outpoints)? {
            match &txout.chain_position {
                ChainPosition::Confirmed { .. } => {
                    if txout.is_confirmed_and_spendable(chain_tip.height) {
                        confirmed += txout.txout.value;
                    } else if !txout.is_mature(chain_tip.height) {
                        immature += txout.txout.value;
                    }
                }
                ChainPosition::Unconfirmed { .. } => {
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

/// Methods are available if the anchor (`A`) can be created from [`TxPosInBlock`].
impl<A, X> TxGraph<A, X>
where
    for<'b> A: Anchor + From<TxPosInBlock<'b>>,
    X: Indexer,
    X::ChangeSet: Default + Merge,
{
    /// Batch insert all transactions of the given `block` of `height`, filtering out those that are
    /// irrelevant.
    ///
    /// Each inserted transaction's anchor will be constructed using [`TxPosInBlock`].
    ///
    /// Relevancy is determined by the internal [`Indexer::is_tx_relevant`] implementation of `X`.
    /// Irrelevant transactions in `txs` will be ignored.
    pub fn apply_block_relevant(
        &mut self,
        block: &Block,
        height: u32,
    ) -> ChangeSet<A, X::ChangeSet> {
        let block_id = BlockId {
            hash: block.block_hash(),
            height,
        };
        let mut changeset = ChangeSet::default();
        for (tx_pos, tx) in block.txdata.iter().enumerate() {
            changeset.merge(ChangeSet {
                indexer: self.index.index_tx(tx),
                ..Default::default()
            });
            if self.index.is_tx_relevant(tx) {
                let anchor = TxPosInBlock {
                    block,
                    block_id,
                    tx_pos,
                };
                changeset.merge(self.insert_tx(tx.clone()));
                changeset.merge(self.insert_anchor(tx.compute_txid(), anchor.into()));
            }
        }
        changeset
    }

    /// Batch insert all transactions of the given `block` of `height`.
    ///
    /// Each inserted transaction's anchor will be constructed using [`TxPosInBlock`].
    ///
    /// To only insert relevant transactions, use [`apply_block_relevant`] instead.
    ///
    /// [`apply_block_relevant`]: Self::apply_block_relevant
    pub fn apply_block(&mut self, block: Block, height: u32) -> ChangeSet<A, X::ChangeSet> {
        let block_id = BlockId {
            hash: block.block_hash(),
            height,
        };
        let mut changeset = ChangeSet::default();
        for (tx_pos, tx) in block.txdata.iter().enumerate() {
            changeset.merge(ChangeSet {
                indexer: self.index.index_tx(tx),
                ..Default::default()
            });
            let anchor = TxPosInBlock {
                block: &block,
                block_id,
                tx_pos,
            };
            changeset.merge(self.insert_tx(tx.clone()));
            changeset.merge(self.insert_anchor(tx.compute_txid(), anchor.into()));
        }
        changeset
    }
}

impl<A, X> AsRef<TxGraph<A, X>> for TxGraph<A, X> {
    fn as_ref(&self) -> &TxGraph<A, X> {
        self
    }
}

/// The [`TxChangeSet`] represents changes to transaction data in [`TxGraph`].
///
/// Transaction data includes full transactions, floating txouts (for fee/feerate calculations),
/// [`Anchor`]s, and timestamps of when the transaction was seen in the mempool (currently only
/// `last-seen`).
///
/// Transaction data is monotone. The `TxChangeSet` can only contain transactions to be added and
/// not removed.
///
/// Refer to [`ChangeSet`] which also includes changes to the internal [`Indexer`] of [`TxGraph`].
/// Refer to the [module-level documentation](crate::tx_graph) for more.
///
/// [`TxGraph`]: crate::TxGraph
/// [`Indexer`]: crate::Indexer
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: Ord + serde::Deserialize<'de>",
        serialize = "A: Ord + serde::Serialize",
    ))
)]
pub struct TxChangeSet<A = ()> {
    /// Added transactions.
    pub txs: BTreeSet<Arc<Transaction>>,
    /// Added txouts.
    pub txouts: BTreeMap<OutPoint, TxOut>,
    /// Added anchors.
    pub anchors: BTreeSet<(A, Txid)>,
    /// Added last-seen unix timestamps of transactions.
    pub last_seen: BTreeMap<Txid, u64>,
}

impl<A> Default for TxChangeSet<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            txouts: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
        }
    }
}

impl<A> TxChangeSet<A> {
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

impl<A: Ord> TxChangeSet<A> {
    /// Transform the [`TxChangeSet`] to have [`Anchor`]s of another type.
    ///
    /// This takes in a closure of signature `FnMut(A) -> A2` which is called for each [`Anchor`] to
    /// transform it.
    pub fn map_anchors<A2: Ord, F>(self, mut f: F) -> TxChangeSet<A2>
    where
        F: FnMut(A) -> A2,
    {
        TxChangeSet {
            txs: self.txs,
            txouts: self.txouts,
            anchors: BTreeSet::<(A2, Txid)>::from_iter(
                self.anchors.into_iter().map(|(a, txid)| (f(a), txid)),
            ),
            last_seen: self.last_seen,
        }
    }
}

impl<A: Ord> Merge for TxChangeSet<A> {
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

/// The [`ChangeSet`] represents changes to a [`TxGraph`] and it's internal [`Indexer`].
///
/// Refer to the [module-level documentation](crate::tx_graph) for more.
///
/// [`TxGraph`]: crate::TxGraph
/// [`Indexer`]: crate::Indexer
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: Ord + serde::Deserialize<'de>, XC: serde::Deserialize<'de>",
        serialize = "A: Ord + serde::Serialize, XC: serde::Serialize",
    ))
)]
#[must_use]
pub struct ChangeSet<A = (), XC = ()> {
    /// Changes to transaction data.
    #[cfg_attr(feature = "serde", serde(alias = "tx_graph"))]
    pub tx_data: TxChangeSet<A>,
    /// Changes to the indexer.
    pub indexer: XC,
}

impl<A, XC: Default> Default for ChangeSet<A, XC> {
    fn default() -> Self {
        Self {
            tx_data: Default::default(),
            indexer: Default::default(),
        }
    }
}

impl<A, XC: Default> From<TxChangeSet<A>> for ChangeSet<A, XC> {
    fn from(tx_graph: TxChangeSet<A>) -> Self {
        Self {
            tx_data: tx_graph,
            ..Default::default()
        }
    }
}

impl<A: Ord, XC> ChangeSet<A, XC> {
    /// Transform the [`ChangeSet`] to have [`Anchor`]s of another type.
    ///
    /// This takes in a closure of signature `FnMut(A) -> A2` which is called for each [`Anchor`] to
    /// transform it.
    pub fn map_anchors<A2: Ord, F>(self, f: F) -> ChangeSet<A2, XC>
    where
        F: FnMut(A) -> A2,
    {
        ChangeSet {
            tx_data: self.tx_data.map_anchors(f),
            indexer: self.indexer,
        }
    }
}

impl<A: Ord, I: Merge> Merge for ChangeSet<A, I> {
    fn merge(&mut self, other: Self) {
        self.tx_data.merge(other.tx_data);
        self.indexer.merge(other.indexer);
    }

    fn is_empty(&self) -> bool {
        self.tx_data.is_empty() && self.indexer.is_empty()
    }
}
/// An iterator that traverses ancestors of a given root transaction.
///
/// The iterator excludes partial transactions.
///
/// Returned by the [`walk_ancestors`] method of [`TxGraph`].
///
/// [`walk_ancestors`]: TxGraph::walk_ancestors
pub struct TxAncestors<'g, A, X, F, O>
where
    F: FnMut(usize, Arc<Transaction>) -> Option<O>,
{
    graph: &'g TxGraph<A, X>,
    visited: HashSet<Txid>,
    queue: VecDeque<(usize, Arc<Transaction>)>,
    filter_map: F,
}

impl<'g, A, X, F, O> TxAncestors<'g, A, X, F, O>
where
    F: FnMut(usize, Arc<Transaction>) -> Option<O>,
{
    /// Creates a `TxAncestors` that includes the starting `Transaction` when iterating.
    pub(crate) fn new_include_root(
        graph: &'g TxGraph<A, X>,
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
        graph: &'g TxGraph<A, X>,
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
        graph: &'g TxGraph<A, X>,
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
        graph: &'g TxGraph<A, X>,
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

    /// Traverse all ancestors that are not filtered out by the provided closure.
    pub fn run_until_finished(self) {
        self.for_each(|_| {})
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

impl<A, X, F, O> Iterator for TxAncestors<'_, A, X, F, O>
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
pub struct TxDescendants<'g, A, X, F, O>
where
    F: FnMut(usize, Txid) -> Option<O>,
{
    graph: &'g TxGraph<A, X>,
    visited: HashSet<Txid>,
    queue: VecDeque<(usize, Txid)>,
    filter_map: F,
}

impl<'g, A, X, F, O> TxDescendants<'g, A, X, F, O>
where
    F: FnMut(usize, Txid) -> Option<O>,
{
    /// Creates a `TxDescendants` that includes the starting `txid` when iterating.
    #[allow(unused)]
    pub(crate) fn new_include_root(graph: &'g TxGraph<A, X>, txid: Txid, filter_map: F) -> Self {
        Self {
            graph,
            visited: Default::default(),
            queue: [(0, txid)].into(),
            filter_map,
        }
    }

    /// Creates a `TxDescendants` that excludes the starting `txid` when iterating.
    pub(crate) fn new_exclude_root(graph: &'g TxGraph<A, X>, txid: Txid, filter_map: F) -> Self {
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
        graph: &'g TxGraph<A, X>,
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
        graph: &'g TxGraph<A, X>,
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

    /// Traverse all descendants that are not filtered out by the provided closure.
    pub fn run_until_finished(self) {
        self.for_each(|_| {})
    }

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

impl<A, F, X, O> Iterator for TxDescendants<'_, A, X, F, O>
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
