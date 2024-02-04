use core::convert::Infallible;

use crate::Append;

/// `Persist` wraps a [`PersistBackend`] (`B`) to create a convenient staging area for changes (`C`)
/// before they are persisted.
///
/// Not all changes to the in-memory representation needs to be written to disk right away, so
/// [`Persist::stage`] can be used to *stage* changes first and then [`Persist::commit`] can be used
/// to write changes to disk.
#[derive(Debug)]
pub struct Persist<B, C> {
    backend: B,
    stage: C,
}

impl<B, C> Persist<B, C>
where
    B: PersistBackend<C>,
    C: Default + Append,
{
    /// Create a new [`Persist`] from [`PersistBackend`].
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            stage: Default::default(),
        }
    }

    /// Stage a `changeset` to be committed later with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, changeset: C) {
        self.stage.append(changeset)
    }

    /// Get the changes that have not been committed yet.
    pub fn staged(&self) -> &C {
        &self.stage
    }

    /// Commit the staged changes to the underlying persistence backend.
    ///
    /// Changes that are committed (if any) are returned.
    ///
    /// # Error
    ///
    /// Returns a backend-defined error if this fails.
    pub fn commit(&mut self) -> Result<Option<C>, B::WriteError> {
        if self.stage.is_empty() {
            return Ok(None);
        }
        self.backend
            .write_changes(&self.stage)
            // if written successfully, take and return `self.stage`
            .map(|_| Some(core::mem::take(&mut self.stage)))
    }

    /// Stages a new changeset and commits it (along with any other previously staged changes) to
    /// the persistence backend
    ///
    /// Convenience method for calling [`stage`] and then [`commit`].
    ///
    /// [`stage`]: Self::stage
    /// [`commit`]: Self::commit
    pub fn stage_and_commit(&mut self, changeset: C) -> Result<Option<C>, B::WriteError> {
        self.stage(changeset);
        self.commit()
    }
}

/// A persistence backend for [`Persist`].
///
/// `C` represents the changeset; a datatype that records changes made to in-memory data structures
/// that are to be persisted, or retrieved from persistence.
pub trait PersistBackend<C> {
    /// The error the backend returns when it fails to write.
    type WriteError: core::fmt::Debug;

    /// The error the backend returns when it fails to load changesets `C`.
    type LoadError: core::fmt::Debug;

    /// Writes a changeset to the persistence backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it inserts the actual changes into a more structured database. All it needs to guarantee is
    /// that [`load_from_persistence`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_from_persistence`]: Self::load_from_persistence
    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError>;

    /// Return the aggregate changeset `C` from persistence.
    fn load_from_persistence(&mut self) -> Result<Option<C>, Self::LoadError>;
}

impl<C> PersistBackend<C> for () {
    type WriteError = Infallible;

    type LoadError = Infallible;

    fn write_changes(&mut self, _changeset: &C) -> Result<(), Self::WriteError> {
        Ok(())
    }

    fn load_from_persistence(&mut self) -> Result<Option<C>, Self::LoadError> {
        Ok(None)
    }
}
