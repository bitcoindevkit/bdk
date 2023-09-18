//! Contains the [`IndexedTxGraph`] structure and associated types.
//!
//! This is essentially a [`TxGraph`] combined with an indexer.

use alloc::vec::Vec;
use bitcoin::{Block, OutPoint, Transaction, TxOut};

use crate::{
    keychain,
    tx_graph::{self, TxGraph},
    Anchor, AnchorFromBlockPosition, Append, BlockId,
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

    /// Batch insert transactions, filtering out those that are irrelevant.
    ///
    /// Relevancy is determined by the [`Indexer::is_tx_relevant`] implementation of `I`. Irrelevant
    /// transactions in `txs` will be ignored. `txs` do not need to be in topological order.
    pub fn batch_insert_relevant<'t>(
        &mut self,
        txs: impl IntoIterator<Item = InsertTxItem<'t, impl IntoIterator<Item = A>>>,
    ) -> ChangeSet<A, I::ChangeSet> {
        // The algorithm below allows for non-topologically ordered transactions by using two loops.
        // This is achieved by:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let mut changeset = ChangeSet::<A, I::ChangeSet>::default();

        let txs = txs
            .into_iter()
            .inspect(|(tx, _, _)| changeset.indexer.append(self.index.index_tx(tx)))
            .collect::<Vec<_>>();

        for (tx, anchors, seen_at) in txs {
            if self.index.is_tx_relevant(tx) {
                changeset.append(self.insert_tx(tx, anchors, seen_at));
            }
        }

        changeset
    }

    /// Batch insert transactions.
    ///
    /// All transactions in `txs` will be inserted. To filter out irrelevant transactions, use
    /// [`batch_insert_relevant`] instead.
    ///
    /// [`batch_insert_relevant`]: IndexedTxGraph::batch_insert_relevant
    pub fn batch_insert<'t>(
        &mut self,
        txs: impl IntoIterator<Item = InsertTxItem<'t, impl IntoIterator<Item = A>>>,
    ) -> ChangeSet<A, I::ChangeSet> {
        let mut changeset = ChangeSet::<A, I::ChangeSet>::default();
        for (tx, anchors, seen_at) in txs {
            changeset.indexer.append(self.index.index_tx(tx));
            changeset.append(self.insert_tx(tx, anchors, seen_at));
        }
        changeset
    }

    /// Batch insert unconfirmed transactions, filtering out those that are irrelevant.
    ///
    /// Relevancy is determined by the internal [`Indexer::is_tx_relevant`] implementation of `I`.
    /// Irrelevant tansactions in `txs` will be ignored.
    ///
    /// Items of `txs` are tuples containing the transaction and an optional *last seen* timestamp.
    /// The *last seen* communicates when the transaction is last seen in the mempool which is used
    /// for conflict-resolution in [`TxGraph`] (refer to [`TxGraph::insert_seen_at`] for details).
    pub fn batch_insert_relevant_unconfirmed<'t>(
        &mut self,
        unconfirmed_txs: impl IntoIterator<Item = (&'t Transaction, Option<u64>)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        self.batch_insert_relevant(
            unconfirmed_txs
                .into_iter()
                .map(|(tx, last_seen)| (tx, core::iter::empty(), last_seen)),
        )
    }

    /// Batch insert unconfirmed transactions.
    ///
    /// Items of `txs` are tuples containing the transaction and an optional *last seen* timestamp.
    /// The *last seen* communicates when the transaction is last seen in the mempool which is used
    /// for conflict-resolution in [`TxGraph`] (refer to [`TxGraph::insert_seen_at`] for details).
    ///
    /// To filter out irrelevant transactions, use [`batch_insert_relevant_unconfirmed`] instead.
    ///
    /// [`batch_insert_relevant_unconfirmed`]: IndexedTxGraph::batch_insert_relevant_unconfirmed
    pub fn batch_insert_unconfirmed<'t>(
        &mut self,
        unconfirmed_txs: impl IntoIterator<Item = (&'t Transaction, Option<u64>)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        self.batch_insert(
            unconfirmed_txs
                .into_iter()
                .map(|(tx, last_seen)| (tx, core::iter::empty(), last_seen)),
        )
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
    /// Irrelevant tansactions in `txs` will be ignored.
    pub fn apply_block_relevant(
        &mut self,
        block: Block,
        height: u32,
    ) -> ChangeSet<A, I::ChangeSet> {
        let block_id = BlockId {
            hash: block.block_hash(),
            height,
        };
        let txs = block.txdata.iter().enumerate().map(|(tx_pos, tx)| {
            (
                tx,
                core::iter::once(A::from_block_position(&block, block_id, tx_pos)),
                None,
            )
        });
        self.batch_insert_relevant(txs)
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
        let txs = block.txdata.iter().enumerate().map(|(tx_pos, tx)| {
            (
                tx,
                core::iter::once(A::from_block_position(&block, block_id, tx_pos)),
                None,
            )
        });
        self.batch_insert(txs)
    }
}

/// A tuple of a transaction, and associated metadata, that are to be inserted into [`IndexedTxGraph`].
///
/// This tuple contains fields in the following order:
/// * A reference to the transaction.
/// * A collection of [`Anchor`]s.
/// * An optional last-seen timestamp.
///
/// This is used as a input item of [`batch_insert_relevant`] and [`batch_insert`].
///
/// [`batch_insert_relevant`]: IndexedTxGraph::batch_insert_relevant
/// [`batch_insert`]: IndexedTxGraph::batch_insert
pub type InsertTxItem<'t, A> = (&'t Transaction, A, Option<u64>);

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
