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

use crate::Append;

/// `Persist` wraps a [`PersistBackend`] to create a convenient staging area for changes before they
/// are persisted. Not all changes made to the [`KeychainTracker`] need to be written to disk right
/// away so you can use [`Persist::stage`] to *stage* it first and then [`Persist::commit`] to
/// finally, write it to disk.
///
/// [`KeychainTracker`]: keychain::KeychainTracker
#[derive(Debug)]
pub struct Persist<A, B, T> {
    backend: B,
    stage: A,
    phanton_data: core::marker::PhantomData<T>
}

impl<A, B, T> Persist<A, B, T> {
    /// Create a new `Persist` from a [`PersistBackend`].
    pub fn new(backend: B) -> Self
    where
        A: Default
    {
        Self {
            backend,
            stage: Default::default(),
            phanton_data: Default::default()
        }
    }

    /// Stage a `changeset` to later persistence with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, addition: A)
    where
        A: Append
    {
        self.stage.append(addition)
    }

    /// Get the changes that haven't been committed yet
    pub fn staged(&self) -> &A {
        &self.stage
    }

    /// Commit the staged changes to the underlying persistence backend.
    ///
    /// Returns a backend-defined error if this fails.
    pub fn commit(&mut self) -> Result<(), B::WriteError>
    where
        B: PersistBackend<A, T>,
        A: Default + Append
    {
        self.backend.append(&self.stage)?;
        self.stage = Default::default();
        Ok(())
    }
}

/// A persistence backend for [`Persist`].
pub trait PersistBackend<A, T>
where
    A: Append
 {
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
    fn append(
        &mut self,
        additions: &A,
    ) -> Result<(), Self::WriteError>;

    /// Applies all the changesets the backend has received to `tracker`.
    fn load(
        &mut self,
        tracker: &mut T,
    ) -> Result<(), Self::LoadError>;
}

impl<A, T> PersistBackend<A, T> for ()
where
    A: Append
{
    type WriteError = ();
    type LoadError = ();

    fn append(
        &mut self,
        _additions: &A,
    ) -> Result<(), Self::WriteError> {
        Ok(())
    }
    fn load(
        &mut self,
        _tracker: &mut T,
    ) -> Result<(), Self::LoadError> {
        Ok(())
    }
}
