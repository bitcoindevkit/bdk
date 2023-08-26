//! Contains the [`IndexedTxGraph`] structure and associated types.
//!
//! This is essentially a [`TxGraph`] combined with an indexer.

use alloc::vec::Vec;
use bitcoin::{OutPoint, Transaction, TxOut};

use crate::{
    keychain,
    tx_graph::{self, TxGraph},
    Anchor, Append,
};

/// A struct that combines [`TxGraph`] and an [`Indexer`] implementation.
///
/// This structure ensures that [`TxGraph`] and [`Indexer`] are updated atomically.
#[derive(Debug)]
pub struct IndexedTxGraph<A, I> {
    /// Transaction index.
    pub index: I,
    graph: TxGraph<A>,
}

impl<A, I: Default> Default for IndexedTxGraph<A, I> {
    fn default() -> Self {
        Self {
            graph: Default::default(),
            index: Default::default(),
        }
    }
}

impl<A, I> IndexedTxGraph<A, I> {
    /// Construct a new [`IndexedTxGraph`] with a given `index`.
    pub fn new(index: I) -> Self {
        Self {
            index,
            graph: TxGraph::default(),
        }
    }

    /// Get a reference of the internal transaction graph.
    pub fn graph(&self) -> &TxGraph<A> {
        &self.graph
    }
}

impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I> {
    /// Applies the [`ChangeSet`] to the [`IndexedTxGraph`].
    pub fn apply_changeset(&mut self, changeset: ChangeSet<A, I::ChangeSet>) {
        self.index.apply_changeset(changeset.indexer);

        for tx in &changeset.graph.txs {
            self.index.index_tx(tx);
        }
        for (&outpoint, txout) in &changeset.graph.txouts {
            self.index.index_txout(outpoint, txout);
        }

        self.graph.apply_changeset(changeset.graph);
    }

    /// Determines the [`ChangeSet`] between `self` and an empty [`IndexedTxGraph`].
    pub fn initial_changeset(&self) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.initial_changeset();
        let indexer = self.index.initial_changeset();
        ChangeSet { graph, indexer }
    }
}

impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I>
where
    I::ChangeSet: Default + Append,
{
    /// Apply an `update` directly.
    ///
    /// `update` is a [`TxGraph<A>`] and the resultant changes is returned as [`ChangeSet`].
    pub fn apply_update(&mut self, update: TxGraph<A>) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.apply_update(update);

        let mut indexer = I::ChangeSet::default();
        for added_tx in &graph.txs {
            indexer.append(self.index.index_tx(added_tx));
        }
        for (&added_outpoint, added_txout) in &graph.txouts {
            indexer.append(self.index.index_txout(added_outpoint, added_txout));
        }

        ChangeSet { graph, indexer }
    }

    /// Insert a floating `txout` of given `outpoint`.
    pub fn insert_txout(
        &mut self,
        outpoint: OutPoint,
        txout: &TxOut,
    ) -> ChangeSet<A, I::ChangeSet> {
        let mut update = TxGraph::<A>::default();
        let _ = update.insert_txout(outpoint, txout.clone());
        self.apply_update(update)
    }

    /// Insert and index a transaction into the graph.
    ///
    /// `anchors` can be provided to anchor the transaction to various blocks. `seen_at` is a
    /// unix timestamp of when the transaction is last seen.
    pub fn insert_tx(
        &mut self,
        tx: &Transaction,
        anchors: impl IntoIterator<Item = A>,
        seen_at: Option<u64>,
    ) -> ChangeSet<A, I::ChangeSet> {
        let txid = tx.txid();

        let mut update = TxGraph::<A>::default();
        if self.graph.get_tx(txid).is_none() {
            let _ = update.insert_tx(tx.clone());
        }
        for anchor in anchors.into_iter() {
            let _ = update.insert_anchor(txid, anchor);
        }
        if let Some(seen_at) = seen_at {
            let _ = update.insert_seen_at(txid, seen_at);
        }

        self.apply_update(update)
    }

    /// Insert relevant transactions from the given `txs` iterator.
    ///
    /// Relevancy is determined by the [`Indexer::is_tx_relevant`] implementation of `I`. Irrelevant
    /// transactions in `txs` will be ignored. `txs` do not need to be in topological order.
    ///
    /// `anchors` can be provided to anchor the transactions to blocks. `seen_at` is a unix
    /// timestamp of when the transactions are last seen.
    pub fn insert_relevant_txs<'t>(
        &mut self,
        txs: impl IntoIterator<Item = TxItem<'t, impl IntoIterator<Item = A>>>,
    ) -> ChangeSet<A, I::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let mut changeset = ChangeSet::<A, I::ChangeSet>::default();
        let mut transactions = Vec::new();
        for (tx, anchors, seen_at) in txs.into_iter() {
            changeset.indexer.append(self.index.index_tx(tx));
            transactions.push((tx, anchors, seen_at));
        }
        changeset.append(
            transactions
                .into_iter()
                .filter_map(
                    |(tx, anchors, seen_at)| match self.index.is_tx_relevant(tx) {
                        true => Some(self.insert_tx(tx, anchors, seen_at)),
                        false => None,
                    },
                )
                .fold(Default::default(), |mut acc, other| {
                    acc.append(other);
                    acc
                }),
        );
        changeset
    }
}

/// Transaction combined with an [`Anchor`] container and an optional last-seen timestamp.
///
/// This is used as a input item of [`IndexedTxGraph::insert_relevant_txs`].
pub type TxItem<'t, A> = (&'t Transaction, A, Option<u64>);

/// A structure that represents changes to an [`IndexedTxGraph`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(
        crate = "serde_crate",
        bound(
            deserialize = "A: Ord + serde::Deserialize<'de>, IA: serde::Deserialize<'de>",
            serialize = "A: Ord + serde::Serialize, IA: serde::Serialize"
        )
    )
)]
#[must_use]
pub struct ChangeSet<A, IA> {
    /// [`TxGraph`] changeset.
    pub graph: tx_graph::ChangeSet<A>,
    /// [`Indexer`] changeset.
    pub indexer: IA,
}

impl<A, IA: Default> Default for ChangeSet<A, IA> {
    fn default() -> Self {
        Self {
            graph: Default::default(),
            indexer: Default::default(),
        }
    }
}

impl<A: Anchor, IA: Append> Append for ChangeSet<A, IA> {
    fn append(&mut self, other: Self) {
        self.graph.append(other.graph);
        self.indexer.append(other.indexer);
    }

    fn is_empty(&self) -> bool {
        self.graph.is_empty() && self.indexer.is_empty()
    }
}

impl<A, IA: Default> From<tx_graph::ChangeSet<A>> for ChangeSet<A, IA> {
    fn from(graph: tx_graph::ChangeSet<A>) -> Self {
        Self {
            graph,
            ..Default::default()
        }
    }
}

impl<A, K> From<keychain::ChangeSet<K>> for ChangeSet<A, keychain::ChangeSet<K>> {
    fn from(indexer: keychain::ChangeSet<K>) -> Self {
        Self {
            graph: Default::default(),
            indexer,
        }
    }
}

/// Utilities for indexing transaction data.
///
/// Types which implement this trait can be used to construct an [`IndexedTxGraph`].
/// This trait's methods should rarely be called directly.
pub trait Indexer {
    /// The resultant "changeset" when new transaction data is indexed.
    type ChangeSet;

    /// Scan and index the given `outpoint` and `txout`.
    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::ChangeSet;

    /// Scans a transaction for relevant outpoints, which are stored and indexed internally.
    fn index_tx(&mut self, tx: &Transaction) -> Self::ChangeSet;

    /// Apply changeset to itself.
    fn apply_changeset(&mut self, changeset: Self::ChangeSet);

    /// Determines the [`ChangeSet`] between `self` and an empty [`Indexer`].
    fn initial_changeset(&self) -> Self::ChangeSet;

    /// Determines whether the transaction should be included in the index.
    fn is_tx_relevant(&self, tx: &Transaction) -> bool;
}
