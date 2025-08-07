//! Contains the [`IndexedTxGraph`] and associated types. Refer to the
//! [`IndexedTxGraph`] documentation for more.
use core::{
    convert::Infallible,
    fmt::{self, Debug},
    ops::RangeBounds,
};

use alloc::{sync::Arc, vec::Vec};
use bitcoin::{Block, OutPoint, ScriptBuf, Transaction, TxOut, Txid};

use crate::{
    spk_txout::SpkTxOutIndex,
    tx_graph::{self, TxGraph},
    Anchor, BlockId, ChainOracle, Indexer, Merge, TxPosInBlock,
};

/// A [`TxGraph<A>`] paired with an indexer `I`, enforcing that every insertion into the graph is
/// simultaneously fed through the indexer.
///
/// This guarantees that `tx_graph` and `index` remain in sync: any transaction or floating txout
/// you add to `tx_graph` has already been processed by `index`.
#[derive(Debug, Clone)]
pub struct IndexedTxGraph<A, I> {
    /// The indexer used for filtering transactions and floating txouts that we are interested in.
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

    // If `tx` replaces a relevant tx, it should also be considered relevant.
    fn is_tx_or_conflict_relevant(&self, tx: &Transaction) -> bool {
        self.index.is_tx_relevant(tx)
            || self
                .graph
                .direct_conflicts(tx)
                .filter_map(|(_, txid)| self.graph.get_tx(txid))
                .any(|tx| self.index.is_tx_relevant(&tx))
    }
}

impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I>
where
    I::ChangeSet: Default + Merge,
{
    /// Create a new, empty [`IndexedTxGraph`].
    ///
    /// The underlying `TxGraph` is initialized with `TxGraph::default()`, and the provided
    /// `index`er is used as‐is (since there are no existing transactions to process).
    pub fn new(index: I) -> Self {
        Self {
            index,
            graph: TxGraph::default(),
        }
    }

    /// Reconstruct an [`IndexedTxGraph`] from persisted graph + indexer state.
    ///
    /// 1. Rebuilds the `TxGraph` from `changeset.tx_graph`.
    /// 2. Calls your `indexer_from_changeset` closure on `changeset.indexer` to restore any state
    ///    your indexer needs beyond its raw changeset.
    /// 3. Runs a full `.reindex()`, returning its `ChangeSet` to describe any additional updates
    ///    applied.
    ///
    /// # Errors
    ///
    /// Returns `Err(E)` if `indexer_from_changeset` fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use bdk_chain::IndexedTxGraph;
    /// # use bdk_chain::indexed_tx_graph::ChangeSet;
    /// # use bdk_chain::indexer::keychain_txout::{KeychainTxOutIndex, DEFAULT_LOOKAHEAD};
    /// # use bdk_core::BlockId;
    /// # use bdk_testenv::anyhow;
    /// # use miniscript::{Descriptor, DescriptorPublicKey};
    /// # use std::str::FromStr;
    /// # let persisted_changeset = ChangeSet::<BlockId, _>::default();
    /// # let persisted_desc = Some(Descriptor::<DescriptorPublicKey>::from_str("")?);
    /// # let persisted_change_desc = Some(Descriptor::<DescriptorPublicKey>::from_str("")?);
    ///
    /// let (graph, reindex_cs) =
    ///     IndexedTxGraph::from_changeset(persisted_changeset, move |idx_cs| -> anyhow::Result<_> {
    ///         // e.g. KeychainTxOutIndex needs descriptors that weren’t in its change set.
    ///         let mut idx = KeychainTxOutIndex::from_changeset(DEFAULT_LOOKAHEAD, true, idx_cs);
    ///         if let Some(desc) = persisted_desc {
    ///             idx.insert_descriptor("external", desc)?;
    ///         }
    ///         if let Some(desc) = persisted_change_desc {
    ///             idx.insert_descriptor("internal", desc)?;
    ///         }
    ///         Ok(idx)
    ///     })?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn from_changeset<F, E>(
        changeset: ChangeSet<A, I::ChangeSet>,
        indexer_from_changeset: F,
    ) -> Result<(Self, ChangeSet<A, I::ChangeSet>), E>
    where
        F: FnOnce(I::ChangeSet) -> Result<I, E>,
    {
        let graph = TxGraph::<A>::from_changeset(changeset.tx_graph);
        let index = indexer_from_changeset(changeset.indexer)?;
        let mut out = Self { graph, index };
        let out_changeset = out.reindex();
        Ok((out, out_changeset))
    }

    /// Synchronizes the indexer to reflect every entry in the transaction graph.
    ///
    /// Iterates over **all** full transactions and floating outputs in `self.graph`, passing each
    /// into `self.index`. Any indexer-side changes produced (via `index_tx` or `index_txout`) are
    /// merged into a fresh `ChangeSet`, which is then returned.
    pub fn reindex(&mut self) -> ChangeSet<A, I::ChangeSet> {
        let mut changeset = ChangeSet::<A, I::ChangeSet>::default();
        for tx in self.graph.full_txs() {
            changeset.indexer.merge(self.index.index_tx(&tx));
        }
        for (op, txout) in self.graph.floating_txouts() {
            changeset.indexer.merge(self.index.index_txout(op, txout));
        }
        changeset
    }

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
    /// `update` is a [`tx_graph::TxUpdate<A>`] and the resultant changes is returned as
    /// [`ChangeSet`].
    pub fn apply_update(&mut self, update: tx_graph::TxUpdate<A>) -> ChangeSet<A, I::ChangeSet> {
        let tx_graph = self.graph.apply_update(update);
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

    /// Inserts the given `evicted_at` for `txid`.
    ///
    /// The `evicted_at` timestamp represents the last known time when the transaction was observed
    /// to be missing from the mempool. If `txid` was previously recorded with an earlier
    /// `evicted_at` value, it is updated only if the new value is greater.
    pub fn insert_evicted_at(&mut self, txid: Txid, evicted_at: u64) -> ChangeSet<A, I::ChangeSet> {
        let tx_graph = self.graph.insert_evicted_at(txid, evicted_at);
        ChangeSet {
            tx_graph,
            ..Default::default()
        }
    }

    /// Batch inserts `(txid, evicted_at)` pairs for `txid`s that the graph is tracking.
    ///
    /// The `evicted_at` timestamp represents the last known time when the transaction was observed
    /// to be missing from the mempool. If `txid` was previously recorded with an earlier
    /// `evicted_at` value, it is updated only if the new value is greater.
    pub fn batch_insert_relevant_evicted_at(
        &mut self,
        evicted_ats: impl IntoIterator<Item = (Txid, u64)>,
    ) -> ChangeSet<A, I::ChangeSet> {
        let tx_graph = self.graph.batch_insert_relevant_evicted_at(evicted_ats);
        ChangeSet {
            tx_graph,
            ..Default::default()
        }
    }

    /// Batch insert transactions, filtering out those that are irrelevant.
    ///
    /// `txs` do not need to be in topological order.
    ///
    /// Relevancy is determined by the internal [`Indexer::is_tx_relevant`] implementation of `I`.
    /// A transaction that conflicts with a relevant transaction is also considered relevant.
    /// Irrelevant transactions in `txs` will be ignored.
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
            if self.is_tx_or_conflict_relevant(&tx) {
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
    /// A transaction that conflicts with a relevant transaction is also considered relevant.
    /// Irrelevant transactions in `unconfirmed_txs` will be ignored.
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
                .filter(|(tx, _)| self.is_tx_or_conflict_relevant(tx))
                .map(|(tx, seen_at)| (tx.clone(), seen_at))
                .collect::<Vec<_>>(),
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
    /// A transaction that conflicts with a relevant transaction is also considered relevant.
    /// Irrelevant transactions in `block` will be ignored.
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
            if self.is_tx_or_conflict_relevant(tx) {
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

impl<A, X> IndexedTxGraph<A, X>
where
    A: Anchor,
{
    /// List txids that are expected to exist under the given spks.
    ///
    /// This is used to fill
    /// [`SyncRequestBuilder::expected_spk_txids`](bdk_core::spk_client::SyncRequestBuilder::expected_spk_txids).
    ///
    ///
    /// The spk index range can be contrained with `range`.
    ///
    /// # Error
    ///
    /// If the [`ChainOracle`] implementation (`chain`) fails, an error will be returned with the
    /// returned item.
    ///
    /// If the [`ChainOracle`] is infallible,
    /// [`list_expected_spk_txids`](Self::list_expected_spk_txids) can be used instead.
    pub fn try_list_expected_spk_txids<'a, C, I>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
        spk_index_range: impl RangeBounds<I> + 'a,
    ) -> impl Iterator<Item = Result<(ScriptBuf, Txid), C::Error>> + 'a
    where
        C: ChainOracle,
        X: AsRef<SpkTxOutIndex<I>> + 'a,
        I: fmt::Debug + Clone + Ord + 'a,
    {
        self.graph
            .try_list_expected_spk_txids(chain, chain_tip, &self.index, spk_index_range)
    }

    /// List txids that are expected to exist under the given spks.
    ///
    /// This is the infallible version of
    /// [`try_list_expected_spk_txids`](Self::try_list_expected_spk_txids).
    pub fn list_expected_spk_txids<'a, C, I>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
        spk_index_range: impl RangeBounds<I> + 'a,
    ) -> impl Iterator<Item = (ScriptBuf, Txid)> + 'a
    where
        C: ChainOracle<Error = Infallible>,
        X: AsRef<SpkTxOutIndex<I>> + 'a,
        I: fmt::Debug + Clone + Ord + 'a,
    {
        self.try_list_expected_spk_txids(chain, chain_tip, spk_index_range)
            .map(|r| r.expect("infallible"))
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

impl<A, IA> From<(tx_graph::ChangeSet<A>, IA)> for ChangeSet<A, IA> {
    fn from((tx_graph, indexer): (tx_graph::ChangeSet<A>, IA)) -> Self {
        Self { tx_graph, indexer }
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
