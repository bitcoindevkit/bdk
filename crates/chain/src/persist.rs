use crate::Append;

/// `Persist` wraps a [`PersistBackend`] (`B`) to create a convenient staging area for changes (`C`)
/// before they are persisted.
///
/// Not all changes to the tracker, which is an in-memory representation of wallet/blockchain
/// data, needs to be written to disk right away, so [`Persist::stage`] can be used to *stage*
/// changes first and then [`Persist::commit`] can be used to write changes to disk.
pub struct Persist<B, C> {
    backend: B,
    stage: C,
}

impl<B, C> Persist<B, C>
where
    B: PersistBackend<C>,
    C: Append + Default,
{
    /// Create a new [`Persist`] from [`PersistBackend`].
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            stage: Default::default(),
        }
    }

    /// Stage a `changeset` to be commited later with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, changeset: C) {
        self.stage.append(changeset)
    }

    /// Get the changes that have not been commited yet.
    pub fn staged(&self) -> &C {
        &self.stage
    }

    /// Commit the staged changes to the underlying persistance backend.
    ///
    /// Returns a backend-defined error if this fails.
    pub fn commit(&mut self) -> Result<(), B::WriteError> {
        let mut temp = C::default();
        core::mem::swap(&mut temp, &mut self.stage);
        self.backend.write_changes(&temp)
    }
}

/// A persistence backend for [`Persist`].
///
/// `C` represents the changeset.
pub trait PersistBackend<C> {
    /// The error the backend returns when it fails to write.
    type WriteError: core::fmt::Debug;

    /// Writes a changeset to the persistence backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it inserts the actual changes into a more structured database. All it needs to guarantee is
    /// that [`load_into_tracker`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_into_tracker`]: LoadablePersistBackend::load_into_tracker
    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError>;
}

/// A trait that extends [`PersistBackend`] to be able to load into a tracker implementation (`T`).
pub trait LoadablePersistBackend<T, C>: PersistBackend<C> {
    /// The error the backend returns when it fails to load.
    type LoadError: core::fmt::Debug;

    /// Loads all data from the persistence backend into `tracker`.
    fn load_into_tracker(&mut self, tracker: &mut T) -> Result<(), Self::LoadError>;
}

impl<C> PersistBackend<C> for () {
    type WriteError = ();

    fn write_changes(&mut self, _changeset: &C) -> Result<(), Self::WriteError> {
        Ok(())
    }
}

impl<T, C> LoadablePersistBackend<T, C> for () {
    type LoadError = ();

    fn load_into_tracker(&mut self, _tracker: &mut T) -> Result<(), Self::LoadError> {
        Ok(())
    }
}
