use core::convert::Infallible;

use alloc::vec::Vec;
use bitcoin::{OutPoint, Script, Transaction, TxOut, Txid};

use crate::{
    keychain::Balance,
    tx_graph::{Additions, CanonicalTx, TxGraph},
    Anchor, Append, BlockId, ChainOracle, FullTxOut, ObservedAs,
};

/// A struct that combines [`TxGraph`] and an [`Indexer`] implementation.
///
/// This structure ensures that [`TxGraph`] and [`Indexer`] are updated atomically.
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

impl<A: Anchor, I: Indexer> IndexedTxGraph<A, I> {
    /// Get a reference of the internal transaction graph.
    pub fn graph(&self) -> &TxGraph<A> {
        &self.graph
    }

    /// Applies the [`IndexedAdditions`] to the [`IndexedTxGraph`].
    pub fn apply_additions(&mut self, additions: IndexedAdditions<A, I::Additions>) {
        let IndexedAdditions {
            graph_additions,
            index_additions,
        } = additions;

        self.index.apply_additions(index_additions);

        for tx in &graph_additions.tx {
            self.index.index_tx(tx);
        }
        for (&outpoint, txout) in &graph_additions.txout {
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
        for added_tx in &graph_additions.tx {
            index_additions.append(self.index.index_tx(added_tx));
        }
        for (&added_outpoint, added_txout) in &graph_additions.txout {
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
    /// transactions in `txs` will be ignored.
    ///
    /// `anchors` can be provided to anchor the transactions to blocks. `seen_at` is a unix
    /// timestamp of when the transactions are last seen.
    pub fn insert_relevant_txs<'t>(
        &mut self,
        txs: impl IntoIterator<Item = &'t Transaction>,
        anchors: impl IntoIterator<Item = A> + Clone,
        seen_at: Option<u64>,
    ) -> IndexedAdditions<A, I::Additions> {
        // As mentioned by @LLFourn: This algorithm requires the transactions to be topologically
        // sorted because most indexers cannot decide whether something is relevant unless you have
        // first inserted its ancestors in the index. We can fix this if we instead do this:
        // 1. insert all txs into the index. If they are irrelevant then that's fine it will just
        //    not store anything about them.
        // 2. decide whether to insert them into the graph depending on whether `is_tx_relevant`
        //    returns true or not. (in a second loop).
        let txs = txs
            .into_iter()
            .inspect(|tx| {
                let _ = self.index.index_tx(tx);
            })
            .collect::<Vec<_>>();
        txs.into_iter()
            .filter_map(|tx| match self.index.is_tx_relevant(tx) {
                true => Some(self.insert_tx(tx, anchors.clone(), seen_at)),
                false => None,
            })
            .fold(Default::default(), |mut acc, other| {
                acc.append(other);
                acc
            })
    }
}

impl<A: Anchor, I: OwnedIndexer> IndexedTxGraph<A, I> {
    pub fn try_list_owned_txs<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<CanonicalTx<'a, Transaction, A>, C::Error>> {
        self.graph
            .full_txs()
            .filter(|node| tx_alters_owned_utxo_set(&self.graph, &self.index, node.txid, node.tx))
            .filter_map(move |tx_node| {
                self.graph
                    .try_get_chain_position(chain, chain_tip, tx_node.txid)
                    .map(|v| {
                        v.map(|observed_as| CanonicalTx {
                            observed_as,
                            node: tx_node,
                        })
                    })
                    .transpose()
            })
    }

    pub fn list_owned_txs<'a, C: ChainOracle<Error = Infallible> + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = CanonicalTx<'a, Transaction, A>> {
        self.try_list_owned_txs(chain, chain_tip)
            .map(|r| r.expect("chain oracle is infallible"))
    }

    pub fn try_list_owned_txouts<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<FullTxOut<ObservedAs<A>>, C::Error>> + 'a {
        self.graph()
            .try_list_chain_txouts(chain, chain_tip)
            .filter(|r| {
                if let Ok(full_txout) = r {
                    if !self.index.is_spk_owned(&full_txout.txout.script_pubkey) {
                        return false;
                    }
                }
                true
            })
    }

    pub fn list_owned_txouts<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = FullTxOut<ObservedAs<A>>> + 'a {
        self.try_list_owned_txouts(chain, chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn try_list_owned_unspents<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<FullTxOut<ObservedAs<A>>, C::Error>> + 'a {
        self.graph()
            .try_list_chain_unspents(chain, chain_tip)
            .filter(|r| {
                if let Ok(full_txout) = r {
                    if !self.index.is_spk_owned(&full_txout.txout.script_pubkey) {
                        return false;
                    }
                }
                true
            })
    }

    pub fn list_owned_unspents<'a, C>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = FullTxOut<ObservedAs<A>>> + 'a
    where
        C: ChainOracle + 'a,
    {
        self.try_list_owned_unspents(chain, chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn try_balance<C, F>(
        &self,
        chain: &C,
        chain_tip: BlockId,
        mut should_trust: F,
    ) -> Result<Balance, C::Error>
    where
        C: ChainOracle,
        F: FnMut(&Script) -> bool,
    {
        let tip_height = chain_tip.anchor_block().height;

        let mut immature = 0;
        let mut trusted_pending = 0;
        let mut untrusted_pending = 0;
        let mut confirmed = 0;

        for res in self.try_list_owned_unspents(chain, chain_tip) {
            let txout = res?;

            match &txout.chain_position {
                ObservedAs::Confirmed(_) => {
                    if txout.is_on_coinbase {
                        if txout.is_mature(tip_height) {
                            confirmed += txout.txout.value;
                        } else {
                            immature += txout.txout.value;
                        }
                    }
                }
                ObservedAs::Unconfirmed(_) => {
                    if should_trust(&txout.txout.script_pubkey) {
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

    pub fn balance<C, F>(&self, chain: &C, chain_tip: BlockId, should_trust: F) -> Balance
    where
        C: ChainOracle<Error = Infallible>,
        F: FnMut(&Script) -> bool,
    {
        self.try_balance(chain, chain_tip, should_trust)
            .expect("error is infallible")
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

/// A trait that extends [`Indexer`] to also index "owned" script pubkeys.
pub trait OwnedIndexer: Indexer {
    /// Determines whether a given script pubkey (`spk`) is owned.
    fn is_spk_owned(&self, spk: &Script) -> bool;
}

fn tx_alters_owned_utxo_set<A, I>(
    graph: &TxGraph<A>,
    index: &I,
    txid: Txid,
    tx: &Transaction,
) -> bool
where
    A: Anchor,
    I: OwnedIndexer,
{
    let prev_spends = (0..tx.input.len() as u32)
        .map(|vout| OutPoint { txid, vout })
        .filter_map(|op| graph.get_txout(op));
    prev_spends
        .chain(&tx.output)
        .any(|txout| index.is_spk_owned(&txout.script_pubkey))
}
