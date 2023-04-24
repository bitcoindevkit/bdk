use core::marker::PhantomData;

use crate::Append;

/// `Persist` wraps a [`PersistBackend`] (`B`) to create a convenient staging area for changes (`C`)
/// to the tracker (`T`) before they are persisted.
///
/// Not all changes to the tracker, which is an in-memory representation of wallet/blockchain
/// data, needs to be written to disk right away, so [`Persist::stage`] can be used to *stage*
/// changes first and then [`Persist::commit`] can be used to write changes to disk.
pub struct Persist<T, C, B> {
    backend: B,
    stage: C,
    marker: PhantomData<T>,
}

impl<T, B> Persist<T, T::ChangeSet, B>
where
    T: Loadable,
    B: PersistBackend<T>,
{
    /// Create a new [`Persist`] from [`PersistBackend`].
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            stage: Default::default(),
            marker: Default::default(),
        }
    }

    /// Stage a `changeset` to be commited later with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, changeset: T::ChangeSet) {
        self.stage.append(changeset)
    }

    /// Get the changes that have not been commited yet.
    pub fn staged(&self) -> &T::ChangeSet {
        &self.stage
    }

    /// Commit the staged changes to the underlying persistance backend.
    ///
    /// Returns a backend-defined error if this fails.
    pub fn commit(&mut self) -> Result<(), B::WriteError> {
        let mut temp = T::ChangeSet::default();
        core::mem::swap(&mut temp, &mut self.stage);
        self.backend.write_changes(&temp)
    }
}

/// A persistence backend for [`Persist`].
///
/// `T` represents the tracker, the in-memory data structure which we wish to persist.
pub trait PersistBackend<T: Loadable> {
    /// The error the backend returns when it fails to write.
    type WriteError: core::fmt::Debug;

    /// The error the backend returns when it fails to load.
    type LoadError: core::fmt::Debug;

    /// Writes a changeset to the persistence backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it inserts the actual changes into a more structured database. All it needs to guarantee is
    /// that [`load_into_tracker`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_into_tracker`]: Self::load_into_tracker
    fn write_changes(&mut self, changeset: &T::ChangeSet) -> Result<(), Self::WriteError>;

    /// Loads all data from the persistence backend into `tracker`.
    fn load_into_tracker(&mut self, tracker: &mut T) -> Result<(), Self::LoadError>;
}

impl<T: Loadable> PersistBackend<T> for () {
    type WriteError = ();
    type LoadError = ();

    fn write_changes(&mut self, _changeset: &T::ChangeSet) -> Result<(), Self::WriteError> {
        Ok(())
    }

    fn load_into_tracker(&mut self, _tracker: &mut T) -> Result<(), Self::LoadError> {
        Ok(())
    }
}

/// A trait that represents a structure which can be loaded with changesets.
pub trait Loadable {
    /// The changeset to be loaded into `self`.
    type ChangeSet: Default + Append;
    /// Loads the `changeset` into `self`.
    fn load_changeset(&mut self, changeset: Self::ChangeSet);
}
