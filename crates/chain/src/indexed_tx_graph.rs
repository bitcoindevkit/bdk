//! Contains the [`IndexedTxGraph`] and associated types. Refer to the
//! [`IndexedTxGraph`] documentation for more.
use core::fmt::Debug;

use alloc::{sync::Arc, vec::Vec};
use bitcoin::{Block, OutPoint, Transaction, TxOut, Txid};

use crate::{
    tx_graph::{self, TxGraph},
    Anchor, BlockId, Indexer, Merge, TxPosInBlock,
};

/// The [`IndexedTxGraph`] combines a [`TxGraph`] and an [`Indexer`] implementation.
///
/// It ensures that [`TxGraph`] and [`Indexer`] are updated atomically.
#[derive(Debug, Clone)]
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

        for tx in &changeset.tx_graph.txs {
            self.index.index_tx(tx);
        }
        for (&outpoint, txout) in &changeset.tx_graph.txouts {
            self.index.index_txout(outpoint, txout);
        }

        self.graph.apply_changeset(changeset.tx_graph);
    }

    /// Determines the [`ChangeSet`] between `self` and an empty [`IndexedTxGraph`].
    pub fn initial_changeset(&self) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.initial_changeset();
        let indexer = self.index.initial_changeset();
        ChangeSet {
            tx_graph: graph,
            indexer,
        }
    }
}

impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I>
where
    I::ChangeSet: Default + Merge,
{
    fn index_tx_graph_changeset(
        &mut self,
        tx_graph_changeset: &tx_graph::ChangeSet<A>,
    ) -> I::ChangeSet {
        let mut changeset = I::ChangeSet::default();
        for added_tx in &tx_graph_changeset.txs {
            changeset.merge(self.index.index_tx(added_tx));
        }
        for (&added_outpoint, added_txout) in &tx_graph_changeset.txouts {
            changeset.merge(self.index.index_txout(added_outpoint, added_txout));
        }
        changeset
    }

    /// Apply an `update` directly.
    ///
    /// `update` is a [`tx_graph::TxUpdate<A>`] and the resultant changes is returned as [`ChangeSet`].
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    pub fn apply_update(&mut self, update: tx_graph::TxUpdate<A>) -> ChangeSet<A, I::ChangeSet> {
        let tx_graph = self.graph.apply_update(update);
        let indexer = self.index_tx_graph_changeset(&tx_graph);
        ChangeSet { tx_graph, indexer }
    }

    /// Apply the given `update` with an optional `seen_at` timestamp.
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
    /// Use [`apply_update`](IndexedTxGraph::apply_update) to have the `seen_at` value automatically
    /// set to the current time.
    pub fn apply_update_at(
        &mut self,
        update: tx_graph::TxUpdate<A>,
        seen_at: Option<u64>,
    ) -> ChangeSet<A, I::ChangeSet> {
        let tx_graph = self.graph.apply_update_at(update, seen_at);
        let indexer = self.index_tx_graph_changeset(&tx_graph);
        ChangeSet { tx_graph, indexer }
    }

    /// Insert a floating `txout` of given `outpoint`.
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.insert_txout(outpoint, txout);
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet {
            tx_graph: graph,
            indexer,
        }
    }

    /// Insert and index a transaction into the graph.
    pub fn insert_tx<T: Into<Arc<Transaction>>>(&mut self, tx: T) -> ChangeSet<A, I::ChangeSet> {
        let tx_graph = self.graph.insert_tx(tx);
        let indexer = self.index_tx_graph_changeset(&tx_graph);
        ChangeSet { tx_graph, indexer }
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
    pub fn batch_insert_relevant<T: Into<Arc<Transaction>>>(
        &mut self,
        txs: impl IntoIterator<Item = (T, impl IntoIterator<Item = A>)>,
    ) -> ChangeSet<A, I::ChangeSet> {
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

        let mut indexer = I::ChangeSet::default();
        for (tx, _) in &txs {
            indexer.merge(self.index.index_tx(tx));
        }

        let mut tx_graph = tx_graph::ChangeSet::default();
        for (tx, anchors) in txs {
            if self.index.is_tx_relevant(&tx) {
                let txid = tx.compute_txid();
                tx_graph.merge(self.graph.insert_tx(tx.clone()));
                for anchor in anchors {
                    tx_graph.merge(self.graph.insert_anchor(txid, anchor));
                }
            }
        }

        ChangeSet { tx_graph, indexer }
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
    ) -> ChangeSet<A, I::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let txs = unconfirmed_txs
            .into_iter()
            .map(|(tx, last_seen)| (<T as Into<Arc<Transaction>>>::into(tx), last_seen))
            .collect::<Vec<_>>();

        let mut indexer = I::ChangeSet::default();
        for (tx, _) in &txs {
            indexer.merge(self.index.index_tx(tx));
        }

        let graph = self.graph.batch_insert_unconfirmed(
            txs.into_iter()
                .filter(|(tx, _)| self.index.is_tx_relevant(tx))
                .map(|(tx, seen_at)| (tx.clone(), seen_at)),
        );

        ChangeSet {
            tx_graph: graph,
            indexer,
        }
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
    pub fn batch_insert_unconfirmed<T: Into<Arc<Transaction>>>(
        &mut self,
        txs: impl IntoIterator<Item = (T, u64)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        let graph = self.graph.batch_insert_unconfirmed(txs);
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet {
            tx_graph: graph,
            indexer,
        }
    }
}

/// Methods are available if the anchor (`A`) can be created from [`TxPosInBlock`].
impl<A, I> IndexedTxGraph<A, I>
where
    I::ChangeSet: Default + Merge,
    for<'b> A: Anchor + From<TxPosInBlock<'b>>,
    I: Indexer,
{
    /// Batch insert all transactions of the given `block` of `height`, filtering out those that are
    /// irrelevant.
    ///
    /// Each inserted transaction's anchor will be constructed using [`TxPosInBlock`].
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
            changeset.indexer.merge(self.index.index_tx(tx));
            if self.index.is_tx_relevant(tx) {
                let txid = tx.compute_txid();
                let anchor = TxPosInBlock {
                    block,
                    block_id,
                    tx_pos,
                }
                .into();
                changeset.tx_graph.merge(self.graph.insert_tx(tx.clone()));
                changeset
                    .tx_graph
                    .merge(self.graph.insert_anchor(txid, anchor));
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
    /// [`apply_block_relevant`]: IndexedTxGraph::apply_block_relevant
    pub fn apply_block(&mut self, block: Block, height: u32) -> ChangeSet<A, I::ChangeSet> {
        let block_id = BlockId {
            hash: block.block_hash(),
            height,
        };
        let mut graph = tx_graph::ChangeSet::default();
        for (tx_pos, tx) in block.txdata.iter().enumerate() {
            let anchor = TxPosInBlock {
                block: &block,
                block_id,
                tx_pos,
            }
            .into();
            graph.merge(self.graph.insert_anchor(tx.compute_txid(), anchor));
            graph.merge(self.graph.insert_tx(tx.clone()));
        }
        let indexer = self.index_tx_graph_changeset(&graph);
        ChangeSet {
            tx_graph: graph,
            indexer,
        }
    }
}

impl<A, I> AsRef<TxGraph<A>> for IndexedTxGraph<A, I> {
    fn as_ref(&self) -> &TxGraph<A> {
        &self.graph
    }
}

/// Represents changes to an [`IndexedTxGraph`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: Ord + serde::Deserialize<'de>, IA: serde::Deserialize<'de>",
        serialize = "A: Ord + serde::Serialize, IA: serde::Serialize"
    ))
)]
#[must_use]
pub struct ChangeSet<A, IA> {
    /// [`TxGraph`] changeset.
    pub tx_graph: tx_graph::ChangeSet<A>,
    /// [`Indexer`] changeset.
    pub indexer: IA,
}

impl<A, IA: Default> Default for ChangeSet<A, IA> {
    fn default() -> Self {
        Self {
            tx_graph: Default::default(),
            indexer: Default::default(),
        }
    }
}

impl<A: Anchor, IA: Merge> Merge for ChangeSet<A, IA> {
    fn merge(&mut self, other: Self) {
        self.tx_graph.merge(other.tx_graph);
        self.indexer.merge(other.indexer);
    }

    fn is_empty(&self) -> bool {
        self.tx_graph.is_empty() && self.indexer.is_empty()
    }
}

impl<A, IA: Default> From<tx_graph::ChangeSet<A>> for ChangeSet<A, IA> {
    fn from(graph: tx_graph::ChangeSet<A>) -> Self {
        Self {
            tx_graph: graph,
            ..Default::default()
        }
    }
}

#[cfg(feature = "miniscript")]
impl<A> From<crate::keychain_txout::ChangeSet> for ChangeSet<A, crate::keychain_txout::ChangeSet> {
    fn from(indexer: crate::keychain_txout::ChangeSet) -> Self {
        Self {
            tx_graph: Default::default(),
            indexer,
        }
    }
}
