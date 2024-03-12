//! Contains the [`IndexedTxGraph`] and associated types. Refer to the
//! [`IndexedTxGraph`] documentation for more.
use alloc::vec::Vec;
use bitcoin::{Block, OutPoint, Transaction, TxOut, Txid};

use crate::{
    keychain,
    tx_graph::{self, TxGraph},
    Anchor, AnchorFromBlockPosition, Append, BlockId,
};

/// The [`IndexedTxGraph`] combines a [`TxGraph`] and an [`Indexer`] implementation.
///
/// It ensures that [`TxGraph`] and [`Indexer`] are updated atomically.
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
    fn index_tx_graph_changeset(
        &mut self,
        tx_graph_changeset: &tx_graph::ChangeSet<A>,
    ) -> I::ChangeSet {
        let mut changeset = I::ChangeSet::default();
        for added_tx in &tx_graph_changeset.txs {
            changeset.append(self.index.index_tx(added_tx));
        }
        for (&added_outpoint, added_txout) in &tx_graph_changeset.txouts {
            changeset.append(self.index.index_txout(added_outpoint, added_txout));
        }
        changeset
    }

    /// Apply an `update` directly.
    ///
    /// `update` is a [`TxGraph<A>`] and the resultant changes is returned as [`ChangeSet`].
    pub fn apply_update(&mut self, update: TxGraph<A>) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.apply_update(update);
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet { graph, indexer }
    }

    /// Insert a floating `txout` of given `outpoint`.
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.insert_txout(outpoint, txout);
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet { graph, indexer }
    }

    /// Insert and index a transaction into the graph.
    pub fn insert_tx(&mut self, tx: Transaction) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.insert_tx(tx);
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet { graph, indexer }
    }

    /// Insert an `anchor` for a given transaction.
    pub fn insert_anchor(&mut self, txid: Txid, anchor: A) -> ChangeSet<A, I::ChangeSet> {
        self.graph.insert_anchor(txid, anchor).into()
    }

    /// Insert a unix timestamp of when a transaction is seen in the mempool.
    ///
    /// This is used for transaction conflict resolution in [`TxGraph`] where the transaction with
    /// the later last-seen is prioritized.
    pub fn insert_seen_at(&mut self, txid: Txid, seen_at: u64) -> ChangeSet<A, I::ChangeSet> {
        self.graph.insert_seen_at(txid, seen_at).into()
    }

    /// Batch insert transactions, filtering out those that are irrelevant.
    ///
    /// Relevancy is determined by the [`Indexer::is_tx_relevant`] implementation of `I`. Irrelevant
    /// transactions in `txs` will be ignored. `txs` do not need to be in topological order.
    pub fn batch_insert_relevant<'t>(
        &mut self,
        txs: impl IntoIterator<Item = (&'t Transaction, impl IntoIterator<Item = A>)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let txs = txs.into_iter().collect::<Vec<_>>();

        let mut indexer = I::ChangeSet::default();
        for (tx, _) in &txs {
            indexer.append(self.index.index_tx(tx));
        }

        let mut graph = tx_graph::ChangeSet::default();
        for (tx, anchors) in txs {
            if self.index.is_tx_relevant(tx) {
                let txid = tx.txid();
                graph.append(self.graph.insert_tx(tx.clone()));
                for anchor in anchors {
                    graph.append(self.graph.insert_anchor(txid, anchor));
                }
            }
        }

        ChangeSet { graph, indexer }
    }

    /// Batch insert unconfirmed transactions, filtering out those that are irrelevant.
    ///
    /// Relevancy is determined by the internal [`Indexer::is_tx_relevant`] implementation of `I`.
    /// Irrelevant transactions in `txs` will be ignored.
    ///
    /// Items of `txs` are tuples containing the transaction and a *last seen* timestamp. The
    /// *last seen* communicates when the transaction is last seen in the mempool which is used for
    /// conflict-resolution in [`TxGraph`] (refer to [`TxGraph::insert_seen_at`] for details).
    pub fn batch_insert_relevant_unconfirmed<'t>(
        &mut self,
        unconfirmed_txs: impl IntoIterator<Item = (&'t Transaction, u64)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let txs = unconfirmed_txs.into_iter().collect::<Vec<_>>();

        let mut indexer = I::ChangeSet::default();
        for (tx, _) in &txs {
            indexer.append(self.index.index_tx(tx));
        }

        let graph = self.graph.batch_insert_unconfirmed(
            txs.into_iter()
                .filter(|(tx, _)| self.index.is_tx_relevant(tx))
                .map(|(tx, seen_at)| (tx.clone(), seen_at)),
        );

        ChangeSet { graph, indexer }
    }

    /// Batch insert unconfirmed transactions.
    ///
    /// Items of `txs` are tuples containing the transaction and a *last seen* timestamp. The
    /// *last seen* communicates when the transaction is last seen in the mempool which is used for
    /// conflict-resolution in [`TxGraph`] (refer to [`TxGraph::insert_seen_at`] for details).
    ///
    /// To filter out irrelevant transactions, use [`batch_insert_relevant_unconfirmed`] instead.
    ///
    /// [`batch_insert_relevant_unconfirmed`]: IndexedTxGraph::batch_insert_relevant_unconfirmed
    pub fn batch_insert_unconfirmed(
        &mut self,
        txs: impl IntoIterator<Item = (Transaction, u64)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.batch_insert_unconfirmed(txs);
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet { graph, indexer }
    }
}

/// Methods are available if the anchor (`A`) implements [`AnchorFromBlockPosition`].
impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I>
where
    I::ChangeSet: Default + Append,
    A: AnchorFromBlockPosition,
{
    /// Batch insert all transactions of the given `block` of `height`, filtering out those that are
    /// irrelevant.
    ///
    /// Each inserted transaction's anchor will be constructed from
    /// [`AnchorFromBlockPosition::from_block_position`].
    ///
    /// Relevancy is determined by the internal [`Indexer::is_tx_relevant`] implementation of `I`.
    /// Irrelevant transactions in `txs` will be ignored.
    pub fn apply_block_relevant(
        &mut self,
        block: &Block,
        height: u32,
    ) -> ChangeSet<A, I::ChangeSet> {
        let block_id = BlockId {
            hash: block.block_hash(),
            height,
        };
        let mut changeset = ChangeSet::<A, I::ChangeSet>::default();
        for (tx_pos, tx) in block.txdata.iter().enumerate() {
            changeset.indexer.append(self.index.index_tx(tx));
            if self.index.is_tx_relevant(tx) {
                let txid = tx.txid();
                let anchor = A::from_block_position(block, block_id, tx_pos);
                changeset.graph.append(self.graph.insert_tx(tx.clone()));
                changeset
                    .graph
                    .append(self.graph.insert_anchor(txid, anchor));
            }
        }
        changeset
    }

    /// Batch insert all transactions of the given `block` of `height`.
    ///
    /// Each inserted transaction's anchor will be constructed from
    /// [`AnchorFromBlockPosition::from_block_position`].
    ///
    /// To only insert relevant transactions, use [`apply_block_relevant`] instead.
    ///
    /// [`apply_block_relevant`]: IndexedTxGraph::apply_block_relevant
    pub fn apply_block(&mut self, block: Block, height: u32) -> ChangeSet<A, I::ChangeSet> {
        let block_id = BlockId {
            hash: block.block_hash(),
            height,
        };
        let mut graph = tx_graph::ChangeSet::default();
        for (tx_pos, tx) in block.txdata.iter().enumerate() {
            let anchor = A::from_block_position(&block, block_id, tx_pos);
            graph.append(self.graph.insert_anchor(tx.txid(), anchor));
            graph.append(self.graph.insert_tx(tx.clone()));
        }
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet { graph, indexer }
    }
}

/// Represents changes to an [`IndexedTxGraph`].
#[derive(Clone, Debug, PartialEq, Eq)]
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
