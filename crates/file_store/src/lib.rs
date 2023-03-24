#![doc = include_str!("../README.md")]
mod file_store;
use bdk_chain::{
    keychain::{KeychainChangeSet, KeychainTracker, PersistBackend},
    sparse_chain::ChainPosition,
    BlockAnchor,
};
pub use file_store::*;

impl<K, A, P> PersistBackend<K, A, P> for KeychainStore<K, A, P>
where
    K: Ord + Clone + core::fmt::Debug,
    A: BlockAnchor,
    P: ChainPosition,
    KeychainChangeSet<K, A, P>: serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = std::io::Error;

    type LoadError = IterError;

    fn append_changeset(
        &mut self,
        changeset: &KeychainChangeSet<K, A, P>,
    ) -> Result<(), Self::WriteError> {
        KeychainStore::append_changeset(self, changeset)
    }

    fn load_into_keychain_tracker(
        &mut self,
        tracker: &mut KeychainTracker<K, A, P>,
    ) -> Result<(), Self::LoadError> {
        KeychainStore::load_into_keychain_tracker(self, tracker)
    }
}
