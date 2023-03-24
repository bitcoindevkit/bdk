//! Persistence for changes made to a [`KeychainTracker`].
//!
//! BDK's [`KeychainTracker`] needs somewhere to persist changes it makes during operation.
//! Operations like giving out a new address are crucial to persist so that next time the
//! application is loaded, it can find transactions related to that address.
//!
//! Note that the [`KeychainTracker`] does not read this persisted data during operation since it
//! always has a copy in memory.
//!
//! [`KeychainTracker`]: crate::keychain::KeychainTracker

use crate::{keychain, sparse_chain::ChainPosition};

/// `Persist` wraps a [`PersistBackend`] to create a convenient staging area for changes before they
/// are persisted. Not all changes made to the [`KeychainTracker`] need to be written to disk right
/// away so you can use [`Persist::stage`] to *stage* it first and then [`Persist::commit`] to
/// finally, write it to disk.
///
/// [`KeychainTracker`]: keychain::KeychainTracker
#[derive(Debug)]
pub struct Persist<K, A, P, B> {
    backend: B,
    stage: keychain::KeychainChangeSet<K, A, P>,
}

impl<K, A, P, B> Persist<K, A, P, B> {
    /// Create a new `Persist` from a [`PersistBackend`].
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            stage: Default::default(),
        }
    }

    /// Stage a `changeset` to later persistence with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, changeset: keychain::KeychainChangeSet<K, A, P>)
    where
        K: Ord,
        P: ChainPosition,
    {
        self.stage.append(changeset)
    }

    /// Get the changes that haven't been committed yet
    pub fn staged(&self) -> &keychain::KeychainChangeSet<K, A, P> {
        &self.stage
    }

    /// Commit the staged changes to the underlying persistence backend.
    ///
    /// Returns a backend-defined error if this fails.
    pub fn commit(&mut self) -> Result<(), B::WriteError>
    where
        B: PersistBackend<K, A, P>,
    {
        self.backend.append_changeset(&self.stage)?;
        self.stage = Default::default();
        Ok(())
    }
}

/// A persistence backend for [`Persist`].
pub trait PersistBackend<K, A, P> {
    /// The error the backend returns when it fails to write.
    type WriteError: core::fmt::Debug;

    /// The error the backend returns when it fails to load.
    type LoadError: core::fmt::Debug;

    /// Appends a new changeset to the persistent backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it inserts the actual changes into a more structured database. All it needs to guarantee is
    /// that [`load_into_keychain_tracker`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_into_keychain_tracker`]: Self::load_into_keychain_tracker
    fn append_changeset(
        &mut self,
        changeset: &keychain::KeychainChangeSet<K, A, P>,
    ) -> Result<(), Self::WriteError>;

    /// Applies all the changesets the backend has received to `tracker`.
    fn load_into_keychain_tracker(
        &mut self,
        tracker: &mut keychain::KeychainTracker<K, A, P>,
    ) -> Result<(), Self::LoadError>;
}

impl<K, A, P> PersistBackend<K, A, P> for () {
    type WriteError = ();
    type LoadError = ();

    fn append_changeset(
        &mut self,
        _changeset: &keychain::KeychainChangeSet<K, A, P>,
    ) -> Result<(), Self::WriteError> {
        Ok(())
    }
    fn load_into_keychain_tracker(
        &mut self,
        _tracker: &mut keychain::KeychainTracker<K, A, P>,
    ) -> Result<(), Self::LoadError> {
        Ok(())
    }
}
