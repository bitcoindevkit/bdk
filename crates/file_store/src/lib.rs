#![doc = include_str!("../README.md")]
mod keychain_file_store;
mod Indexed_file_store;
use Indexed_file_store::IndexedTxGraphStore;
use bdk_chain::{
    keychain::{KeychainChangeSet, KeychainTracker, PersistBackendOld},
    sparse_chain::ChainPosition, BlockAnchor, Append, Empty, TxIndex, indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
};

use bdk_chain::persist::PersistBackend;
use keychain_file_store::{KeychainStore, IterError};

impl<K, P> PersistBackendOld<K, P> for KeychainStore<K, P>
where
    K: Ord + Clone + core::fmt::Debug,
    P: ChainPosition,
    KeychainChangeSet<K, P>: serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = std::io::Error;

    type LoadError = IterError;

    fn append_changeset(
        &mut self,
        changeset: &KeychainChangeSet<K, P>,
    ) -> Result<(), Self::WriteError> {
        KeychainStore::append_changeset(self, changeset)
    }

    fn load_into_keychain_tracker(
        &mut self,
        tracker: &mut KeychainTracker<K, P>,
    ) -> Result<(), Self::LoadError> {
        KeychainStore::load_into_keychain_tracker(self, tracker)
    }
}

impl<BA, A, T> PersistBackend<A, T> for IndexedTxGraphStore<BA, A, T>
where
    BA: BlockAnchor,
    A: Append + Default + Empty,
    T: TxIndex + TxIndex<Additions = A>,
    IndexedAdditions<BA, A>: serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = std::io::Error;

    type LoadError = Indexed_file_store::IterError;

    fn append(
        &mut self,
        changeset: &A,
    ) -> Result<(), Self::WriteError> {
        IndexedTxGraphStore::append_addition(self, changeset)
    }

    fn load(
        &mut self,
        tracker: &mut T,
    ) -> Result<(), Self::LoadError> {
        IndexedTxGraphStore::load_into_indexed_tx_graph(self, tracker)
    }
}

