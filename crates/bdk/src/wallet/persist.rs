//! Persistence for changes made to a [`Wallet`].
//!
//! BDK's [`Wallet`] needs somewhere to persist changes it makes during operation.
//! Operations like giving out a new address are crucial to persist so that next time the
//! application is loaded it can find transactions related to that address.
//!
//! Note that `Wallet` does not read this persisted data during operation since it always has a copy
//! in memory
use crate::KeychainKind;
use bdk_chain::{keychain::KeychainTracker, ConfirmationTime};

/// `Persist` wraps a [`Backend`] to create a convienient staging area for changes before they are
/// persisted. Not all changes made to the [`Wallet`] need to be written to disk right away so you
/// can use [`Persist::stage`] to *stage* it first and then [`Persist::commit`] to finally write it
/// to disk.
#[derive(Debug)]
pub struct Persist<P> {
    backend: P,
    stage: ChangeSet,
}

impl<P> Persist<P> {
    /// Create a new `Persist` from a [`Backend`]
    pub fn new(backend: P) -> Self {
        Self {
            backend,
            stage: Default::default(),
        }
    }

    /// Stage a `changeset` to later persistence with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, changeset: ChangeSet) {
        self.stage.append(changeset)
    }

    /// Get the changes that haven't been commited yet
    pub fn staged(&self) -> &ChangeSet {
        &self.stage
    }

    /// Commit the staged changes to the underlying persistence backend.
    ///
    /// Retuns a backend defined error if this fails
    pub fn commit(&mut self) -> Result<(), P::WriteError>
    where
        P: Backend,
    {
        self.backend.append_changeset(&self.stage)?;
        self.stage = Default::default();
        Ok(())
    }
}

/// A persistence backend for [`Wallet`]
///
/// [`Wallet`]: crate::Wallet
pub trait Backend {
    /// The error the backend returns when it fails to write
    type WriteError: core::fmt::Debug;
    /// The error the backend returns when it fails to load
    type LoadError: core::fmt::Debug;
    /// Appends a new changeset to the persistance backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it insert the actual changes to a more structured database. All it needs to guarantee is
    /// that [`load_into_keychain_tracker`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_into_keychain_tracker`]: Self::load_into_keychain_tracker
    fn append_changeset(&mut self, changeset: &ChangeSet) -> Result<(), Self::WriteError>;

    /// Applies all the changesets the backend has received to `tracker`.
    fn load_into_keychain_tracker(
        &mut self,
        tracker: &mut KeychainTracker<KeychainKind, ConfirmationTime>,
    ) -> Result<(), Self::LoadError>;
}

#[cfg(feature = "file-store")]
mod file_store {
    use super::*;
    use bdk_chain::file_store::{IterError, KeychainStore};

    type FileStore = KeychainStore<KeychainKind, ConfirmationTime>;

    impl Backend for FileStore {
        type WriteError = std::io::Error;
        type LoadError = IterError;
        fn append_changeset(&mut self, changeset: &ChangeSet) -> Result<(), Self::WriteError> {
            self.append_changeset(changeset)
        }
        fn load_into_keychain_tracker(
            &mut self,
            tracker: &mut KeychainTracker<KeychainKind, ConfirmationTime>,
        ) -> Result<(), Self::LoadError> {
            self.load_into_keychain_tracker(tracker)
        }
    }
}

impl Backend for () {
    type WriteError = ();
    type LoadError = ();
    fn append_changeset(&mut self, _changeset: &ChangeSet) -> Result<(), Self::WriteError> {
        Ok(())
    }
    fn load_into_keychain_tracker(
        &mut self,
        _tracker: &mut KeychainTracker<KeychainKind, ConfirmationTime>,
    ) -> Result<(), Self::LoadError> {
        Ok(())
    }
}

#[cfg(feature = "file-store")]
pub use file_store::*;

use super::ChangeSet;
