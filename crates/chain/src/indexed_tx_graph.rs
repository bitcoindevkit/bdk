use alloc::vec::Vec;
use bitcoin::{OutPoint, Transaction, TxOut};

use crate::{
    keychain::DerivationAdditions,
    tx_graph::{Additions, TxGraph},
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
    /// Applies the [`IndexedAdditions`] to the [`IndexedTxGraph`].
    pub fn apply_additions(&mut self, additions: IndexedAdditions<A, I::Additions>) {
        let IndexedAdditions {
            graph_additions,
            index_additions,
        } = additions;

        self.index.apply_additions(index_additions);

        for tx in &graph_additions.txs {
            self.index.index_tx(tx);
        }
        for (&outpoint, txout) in &graph_additions.txouts {
            self.index.index_txout(outpoint, txout);
        }

        self.graph.apply_additions(graph_additions);
    }
}

impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I>
where
    I::Additions: Default + Append,
{
    /// Apply an `update` directly.
    ///
    /// `update` is a [`TxGraph<A>`] and the resultant changes is returned as [`IndexedAdditions`].
    pub fn apply_update(&mut self, update: TxGraph<A>) -> IndexedAdditions<A, I::Additions> {
        let graph_additions = self.graph.apply_update(update);

        let mut index_additions = I::Additions::default();
        for added_tx in &graph_additions.txs {
            index_additions.append(self.index.index_tx(added_tx));
        }
        for (&added_outpoint, added_txout) in &graph_additions.txouts {
            index_additions.append(self.index.index_txout(added_outpoint, added_txout));
        }

        IndexedAdditions {
            graph_additions,
            index_additions,
        }
    }

    /// Insert a floating `txout` of given `outpoint`.
    pub fn insert_txout(
        &mut self,
        outpoint: OutPoint,
        txout: &TxOut,
    ) -> IndexedAdditions<A, I::Additions> {
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
    ) -> IndexedAdditions<A, I::Additions> {
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
        txs: impl IntoIterator<Item = (&'t Transaction, impl IntoIterator<Item = A>)>,
        seen_at: Option<u64>,
    ) -> IndexedAdditions<A, I::Additions> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let mut additions = IndexedAdditions::<A, I::Additions>::default();
        let mut transactions = Vec::new();
        for (tx, anchors) in txs.into_iter() {
            additions.index_additions.append(self.index.index_tx(tx));
            transactions.push((tx, anchors));
        }
        additions.append(
            transactions
                .into_iter()
                .filter_map(|(tx, anchors)| match self.index.is_tx_relevant(tx) {
                    true => Some(self.insert_tx(tx, anchors, seen_at)),
                    false => None,
                })
                .fold(Default::default(), |mut acc, other| {
                    acc.append(other);
                    acc
                }),
        );
        additions
    }
}

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
pub struct IndexedAdditions<A, IA> {
    /// [`TxGraph`] additions.
    pub graph_additions: Additions<A>,
    /// [`Indexer`] additions.
    pub index_additions: IA,
}

impl<A, IA: Default> Default for IndexedAdditions<A, IA> {
    fn default() -> Self {
        Self {
            graph_additions: Default::default(),
            index_additions: Default::default(),
        }
    }
}

impl<A: Anchor, IA: Append> Append for IndexedAdditions<A, IA> {
    fn append(&mut self, other: Self) {
        self.graph_additions.append(other.graph_additions);
        self.index_additions.append(other.index_additions);
    }

    fn is_empty(&self) -> bool {
        self.graph_additions.is_empty() && self.index_additions.is_empty()
    }
}

impl<A, IA: Default> From<Additions<A>> for IndexedAdditions<A, IA> {
    fn from(graph_additions: Additions<A>) -> Self {
        Self {
            graph_additions,
            ..Default::default()
        }
    }
}

impl<A, K> From<DerivationAdditions<K>> for IndexedAdditions<A, DerivationAdditions<K>> {
    fn from(index_additions: DerivationAdditions<K>) -> Self {
        Self {
            graph_additions: Default::default(),
            index_additions,
        }
    }
}

/// Represents a structure that can index transaction data.
pub trait Indexer {
    /// The resultant "additions" when new transaction data is indexed.
    type Additions;

    /// Scan and index the given `outpoint` and `txout`.
    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::Additions;

    /// Scan and index the given transaction.
    fn index_tx(&mut self, tx: &Transaction) -> Self::Additions;

    /// Apply additions to itself.
    fn apply_additions(&mut self, additions: Self::Additions);

    /// Determines whether the transaction should be included in the index.
    fn is_tx_relevant(&self, tx: &Transaction) -> bool;
}
