mod file_store;
use bdk_chain::{
    bitcoin::Transaction,
    keychain::{KeychainChangeSet, KeychainTracker, PersistBackend},
    sparse_chain::ChainPosition,
};
pub use file_store::*;

impl<'de, K, P> PersistBackend<K, P> for KeychainStore<K, P>
where
    K: Ord + Clone + core::fmt::Debug,
    P: ChainPosition,
    KeychainChangeSet<K, P, Transaction>: serde::Serialize + serde::de::DeserializeOwned,
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
