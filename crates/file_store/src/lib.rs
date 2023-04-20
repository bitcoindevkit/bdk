#![doc = include_str!("../README.md")]
mod entry_iter;
mod keychain_store;
mod store;
use std::io;

use bdk_chain::{
    keychain::{KeychainChangeSet, KeychainTracker, PersistBackend},
    sparse_chain::ChainPosition,
};
use bincode::{DefaultOptions, Options};
pub use entry_iter::*;
pub use keychain_store::*;
pub use store::*;

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

/// Error that occurs due to problems encountered with the file.
#[derive(Debug)]
pub enum FileError {
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes {
        got: Vec<u8>,
        expected: &'static [u8],
    },
}

impl core::fmt::Display for FileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error trying to read file: {}", e),
            Self::InvalidMagicBytes { got, expected } => write!(
                f,
                "file has invalid magic bytes: expected={:?} got={:?}",
                expected, got,
            ),
        }
    }
}

impl From<io::Error> for FileError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl std::error::Error for FileError {}
