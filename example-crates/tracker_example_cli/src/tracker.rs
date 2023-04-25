use std::{convert::Infallible, fmt::Debug};

use bdk_chain::{
    bitcoin::Transaction,
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{DerivationAdditions, KeychainTxOutIndex},
    local_chain::{self, LocalChain},
    tx_graph::CanonicalTx,
    Anchor, Append, BlockId, ChainOracle, FullTxOut, Loadable, ObservedAs,
};
use bdk_file_store::Store;

/// Structure for persisting [`Tracker`] data.
pub type TrackerStore<A, K, C> = Store<Tracker<A, K, C>>;

/// An in-memory representation of chain data that we are tracking.
///
/// * `A` is the [`Anchor`] implementation.
/// * `K` is our keychain identifier.
/// * `C` is the representation of the best chain history. This can either be a [`LocalChain`] or a
/// remote [`ChainOracle`] implementation.
///
/// [`Tracker`] can be constructed with [`new_local`] or [`new_remote`] (depending on the
/// chain-history type).
///
/// [`new_local`]: Self::new_local
/// [`new_remote`]: Self::new_remote
pub struct Tracker<A, K, C = LocalChain> {
    pub indexed_graph: IndexedTxGraph<A, KeychainTxOutIndex<K>>,
    pub chain: C,
}

impl<A, K> Tracker<A, K, LocalChain> {
    /// New [`Tracker`] with a [`LocalChain`] as the best-chain representation.
    pub fn new_local() -> Self {
        Self {
            indexed_graph: Default::default(),
            chain: LocalChain::default(),
        }
    }
}

impl<A, K, O: ChainOracle> Tracker<A, K, RemoteChain<O>> {
    /// New [`Tracker`] with a remote [`ChainOracle`] as the best-chain representation.
    pub fn new_remote(oracle: O) -> Self {
        Self {
            indexed_graph: Default::default(),
            chain: RemoteChain {
                oracle,
                last_seen_height: None,
            },
        }
    }
}

impl<A: Anchor, K: Clone + Ord + Debug, C: ChainOracle> Tracker<A, K, C> {
    pub fn try_list_owned_txouts(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), C::Error>> {
        self.indexed_graph
            .graph()
            .try_list_chain_txouts(&self.chain, chain_tip)
            .filter_map(|r| match r {
                Err(err) => Some(Err(err)),
                Ok(full_txo) => Some(Ok((
                    self.indexed_graph
                        .index
                        .index_of_spk(&full_txo.txout.script_pubkey)?,
                    full_txo,
                ))),
            })
    }

    pub fn try_list_owned_unspents(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), C::Error>> {
        self.try_list_owned_txouts(chain_tip).filter(|r| {
            if let Ok((_, full_txo)) = r {
                if full_txo.spent_by.is_some() {
                    return false;
                }
            }
            true
        })
    }

    pub fn try_list_txs(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<CanonicalTx<Transaction, A>, C::Error>> {
        self.indexed_graph
            .graph()
            .try_list_chain_txs(&self.chain, chain_tip)
    }
}

impl<A: Anchor, K: Clone + Ord + Debug, C: ChainOracle<Error = Infallible>> Tracker<A, K, C> {
    pub fn list_owned_txouts(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = (&(K, u32), FullTxOut<ObservedAs<A>>)> {
        self.try_list_owned_txouts(chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn list_owned_unspents(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = (&(K, u32), FullTxOut<ObservedAs<A>>)> {
        self.try_list_owned_unspents(chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn list_txs(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = CanonicalTx<Transaction, A>> {
        self.try_list_txs(chain_tip)
            .map(|r| r.expect("oracle is infallible"))
    }
}

impl<A: Anchor, K: Default + Clone + Ord + Debug, C: Loadable> Loadable for Tracker<A, K, C> {
    type ChangeSet = ChangeSet<A, K, C::ChangeSet>;

    fn load_changeset(&mut self, changeset: Self::ChangeSet) {
        self.indexed_graph
            .apply_additions(changeset.indexed_graph_additions);
        self.chain.load_changeset(changeset.chain_changeset);
    }
}

#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(bound(
    deserialize = "A: Ord + serde::Deserialize<'de>, K: Ord + serde::Deserialize<'de>, C: Ord + serde::Deserialize<'de>",
    serialize = "A: Ord + serde::Serialize, K: Ord + serde::Serialize, C: Ord + serde::Serialize",
))]
pub struct ChangeSet<A, K, C = local_chain::ChangeSet> {
    pub indexed_graph_additions: IndexedAdditions<A, DerivationAdditions<K>>,
    pub chain_changeset: C,
}

impl<A, K, C: Default> Default for ChangeSet<A, K, C> {
    fn default() -> Self {
        Self {
            indexed_graph_additions: Default::default(),
            chain_changeset: Default::default(),
        }
    }
}

impl<A: Anchor, K: Ord, C: Append> Append for ChangeSet<A, K, C> {
    fn append(&mut self, other: Self) {
        Append::append(
            &mut self.indexed_graph_additions,
            other.indexed_graph_additions,
        );
        Append::append(&mut self.chain_changeset, other.chain_changeset)
    }
}

impl<A, K, C: Default> From<IndexedAdditions<A, DerivationAdditions<K>>> for ChangeSet<A, K, C> {
    fn from(inner_additions: IndexedAdditions<A, DerivationAdditions<K>>) -> Self {
        Self {
            indexed_graph_additions: inner_additions,
            chain_changeset: Default::default(),
        }
    }
}

impl<A, K, C: Default> From<DerivationAdditions<K>> for ChangeSet<A, K, C> {
    fn from(index_additions: DerivationAdditions<K>) -> Self {
        Self {
            indexed_graph_additions: IndexedAdditions {
                graph_additions: Default::default(),
                index_additions,
            },
            chain_changeset: Default::default(),
        }
    }
}

impl<A, K> From<local_chain::ChangeSet> for ChangeSet<A, K, local_chain::ChangeSet> {
    fn from(chain_changeset: local_chain::ChangeSet) -> Self {
        Self {
            indexed_graph_additions: Default::default(),
            chain_changeset,
        }
    }
}

impl<A, K> From<Option<u32>> for ChangeSet<A, K, Option<u32>> {
    fn from(chain_changeset: Option<u32>) -> Self {
        Self {
            indexed_graph_additions: Default::default(),
            chain_changeset,
        }
    }
}

/// Contains a remote best-chain representation alongside the last-seen block's height.
///
/// The last-seen block height is persisted locally and can be used to determine which height to
/// start syncing from for block-by-block chain sources.
pub struct RemoteChain<O> {
    oracle: O,
    last_seen_height: Option<u32>,
}

impl<O> RemoteChain<O> {
    pub fn inner(&self) -> &O {
        &self.oracle
    }

    pub fn last_seen_height(&self) -> Option<u32> {
        self.last_seen_height
    }

    pub fn update_last_seen_height(&mut self, last_seen_height: Option<u32>) -> Option<u32> {
        if self.last_seen_height < last_seen_height {
            self.last_seen_height = last_seen_height;
            last_seen_height
        } else {
            None
        }
    }
}

impl<O> Loadable for RemoteChain<O> {
    type ChangeSet = Option<u32>;

    fn load_changeset(&mut self, changeset: Self::ChangeSet) {
        self.last_seen_height.append(changeset)
    }
}

impl<O: ChainOracle> ChainOracle for RemoteChain<O> {
    type Error = O::Error;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        static_block: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        self.oracle.is_block_in_chain(block, static_block)
    }
}
