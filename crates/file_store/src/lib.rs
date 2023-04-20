#![doc = include_str!("../README.md")]
mod entry_iter;
mod keychain_store;
use bdk_chain::{
    keychain::{KeychainChangeSet, KeychainTracker, PersistBackend},
    sparse_chain::ChainPosition,
};
use bincode::{DefaultOptions, Options};
pub use entry_iter::*;
pub use keychain_store::*;

pub(crate) fn bincode_options() -> impl bincode::Options {
    DefaultOptions::new().with_varint_encoding()
}

impl<K, P> PersistBackend<K, P> for KeychainStore<K, P>
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
