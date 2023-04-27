use std::{convert::Infallible, fmt::Debug};

use bdk_chain::{
    bitcoin::Transaction,
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{DerivationAdditions, KeychainTxOutIndex},
    local_chain::{self, LocalChain},
    tx_graph::CanonicalTx,
    Anchor, Append, BlockId, ChainOracle, FullTxOut, ObservedAs, PersistBackend,
};
use bdk_file_store::{IterError, Store};

use crate::{RemoteChain, RemoteChainChangeSet};

/// Structure for persisting [`Tracker`] data.
pub type TrackerStore<K, A, B, C> = Store<Tracker<K, A, B>, ChangeSet<K, A, C>>;

pub type LocalTracker<K, A> = Tracker<K, A, LocalChain>;
pub type LocalTrackerStore<K, A> = TrackerStore<K, A, LocalChain, local_chain::ChangeSet>;
pub type LocalTrackerChangeSet<K, A> = ChangeSet<K, A, local_chain::ChangeSet>;

pub type RemoteTracker<K, A, O> = Tracker<K, A, RemoteChain<O>>;
pub type RemoteTrackerStore<K, A, O> = TrackerStore<K, A, RemoteChain<O>, RemoteChainChangeSet>;
pub type RemoteTrackerChangeSet<K, A> = ChangeSet<K, A, RemoteChainChangeSet>;

/// An in-memory representation of chain data that we are tracking.
///
/// * `A` is the [`Anchor`] implementation.
/// * `K` is our keychain identifier.
/// * `B` is the representation of the best chain history. This can either be a [`LocalChain`] or a
/// [`RemoteChain`] (which wraps a remote [`ChainOracle`] implementation).
///
/// [`Tracker`] can be constructed with [`new_local`] or [`new_remote`] (depending on the
/// chain-history type).
///
/// [`new_local`]: Self::new_local
/// [`new_remote`]: Self::new_remote
pub struct Tracker<K, A, B> {
    pub indexed_graph: IndexedTxGraph<A, KeychainTxOutIndex<K>>,
    pub chain: B,
}

impl<K, A> LocalTracker<K, A> {
    /// New [`Tracker`] with a [`LocalChain`] as the best-chain representation.
    pub fn new_local() -> Self {
        Self {
            indexed_graph: Default::default(),
            chain: LocalChain::default(),
        }
    }
}

impl<K, A, O: ChainOracle> RemoteTracker<K, A, O> {
    /// New [`Tracker`] with a remote [`ChainOracle`] as the best-chain representation.
    pub fn new_remote(oracle: O) -> Self {
        Self {
            indexed_graph: Default::default(),
            chain: RemoteChain::new(oracle),
        }
    }
}

impl<K, A: Anchor, B: ChainOracle> Tracker<K, A, B>
where
    K: Clone + Ord + Debug,
{
    pub fn try_list_owned_txouts(
        &self,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), B::Error>> {
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
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), B::Error>> {
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
    ) -> impl Iterator<Item = Result<CanonicalTx<Transaction, A>, B::Error>> {
        self.indexed_graph
            .graph()
            .try_list_chain_txs(&self.chain, chain_tip)
    }
}

impl<K, A: Anchor, B: ChainOracle<Error = Infallible>> Tracker<K, A, B>
where
    K: Clone + Ord + Debug,
{
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

impl<K, A> PersistBackend<LocalTracker<K, A>, LocalTrackerChangeSet<K, A>>
    for LocalTrackerStore<K, A>
where
    K: Clone + Ord + Debug + serde::Serialize + serde::de::DeserializeOwned,
    A: Anchor + serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = std::io::Error;

    type LoadError = IterError;

    fn write_changes(
        &mut self,
        changeset: &LocalTrackerChangeSet<K, A>,
    ) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_into_tracker(
        &mut self,
        tracker: &mut LocalTracker<K, A>,
    ) -> Result<(), Self::LoadError> {
        let (changeset, result) = self.aggregate_changesets();
        tracker
            .indexed_graph
            .apply_additions(changeset.indexed_graph_additions);
        tracker.chain.apply_changeset(changeset.chain_changeset);
        result
    }
}

impl<K, A, O> PersistBackend<RemoteTracker<K, A, O>, RemoteTrackerChangeSet<K, A>>
    for RemoteTrackerStore<K, A, O>
where
    K: Clone + Ord + Debug + serde::Serialize + serde::de::DeserializeOwned,
    A: Anchor + serde::Serialize + serde::de::DeserializeOwned,
    O: ChainOracle,
{
    type WriteError = std::io::Error;

    type LoadError = IterError;

    fn write_changes(
        &mut self,
        changeset: &RemoteTrackerChangeSet<K, A>,
    ) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_into_tracker(
        &mut self,
        tracker: &mut RemoteTracker<K, A, O>,
    ) -> Result<(), Self::LoadError> {
        let (changeset, result) = self.aggregate_changesets();
        tracker
            .indexed_graph
            .apply_additions(changeset.indexed_graph_additions);
        tracker.chain.apply_changeset(changeset.chain_changeset);
        result
    }
}

/// A structure that represents changes to [`Tracker`].
#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(bound(
    deserialize = "A: Ord + serde::Deserialize<'de>, K: Ord + serde::Deserialize<'de>, C: Ord + serde::Deserialize<'de>",
    serialize = "A: Ord + serde::Serialize, K: Ord + serde::Serialize, C: Ord + serde::Serialize",
))]
pub struct ChangeSet<K, A, C> {
    pub indexed_graph_additions: IndexedAdditions<A, DerivationAdditions<K>>,
    pub chain_changeset: C,
}

impl<K, A, C: Default> Default for ChangeSet<K, A, C> {
    fn default() -> Self {
        Self {
            indexed_graph_additions: Default::default(),
            chain_changeset: Default::default(),
        }
    }
}

impl<K: Ord, A: Anchor, C: Append> Append for ChangeSet<K, A, C> {
    fn append(&mut self, other: Self) {
        Append::append(
            &mut self.indexed_graph_additions,
            other.indexed_graph_additions,
        );
        Append::append(&mut self.chain_changeset, other.chain_changeset)
    }
}

impl<K, A, C: Default> From<IndexedAdditions<A, DerivationAdditions<K>>> for ChangeSet<K, A, C> {
    fn from(inner_additions: IndexedAdditions<A, DerivationAdditions<K>>) -> Self {
        Self {
            indexed_graph_additions: inner_additions,
            chain_changeset: Default::default(),
        }
    }
}

impl<K, A, C: Default> From<DerivationAdditions<K>> for ChangeSet<K, A, C> {
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

impl<K, A> From<local_chain::ChangeSet> for ChangeSet<K, A, local_chain::ChangeSet> {
    fn from(chain_changeset: local_chain::ChangeSet) -> Self {
        Self {
            indexed_graph_additions: Default::default(),
            chain_changeset,
        }
    }
}

impl<K, A> From<Option<u32>> for ChangeSet<K, A, Option<u32>> {
    fn from(chain_changeset: Option<u32>) -> Self {
        Self {
            indexed_graph_additions: Default::default(),
            chain_changeset,
        }
    }
}
