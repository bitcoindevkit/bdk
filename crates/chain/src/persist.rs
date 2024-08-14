use core::{
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
};

use alloc::boxed::Box;

use crate::Merge;

/// Represents a type that contains staged changes.
pub trait Staged {
    /// Type for staged changes.
    type ChangeSet: Merge;

    /// Get mutable reference of staged changes.
    fn staged(&mut self) -> &mut Self::ChangeSet;
}

/// Trait that persists the type with `Db`.
///
/// Methods of this trait should not be called directly.
pub trait PersistWith<Db>: Staged + Sized {
    /// Parameters for [`PersistWith::create`].
    type CreateParams;
    /// Parameters for [`PersistWith::load`].
    type LoadParams;
    /// Error type of [`PersistWith::create`].
    type CreateError;
    /// Error type of [`PersistWith::load`].
    type LoadError;
    /// Error type of [`PersistWith::persist`].
    type PersistError;

    /// Initialize the `Db` and create `Self`.
    fn create(db: &mut Db, params: Self::CreateParams) -> Result<Self, Self::CreateError>;

    /// Initialize the `Db` and load a previously-persisted `Self`.
    fn load(db: &mut Db, params: Self::LoadParams) -> Result<Option<Self>, Self::LoadError>;

    /// Persist changes to the `Db`.
    fn persist(
        db: &mut Db,
        changeset: &<Self as Staged>::ChangeSet,
    ) -> Result<(), Self::PersistError>;
}

pub type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Trait that persists the type with an async `Db`.
#[async_trait::async_trait]
pub trait PersistAsyncWith<Db>: Staged + Sized {
    /// Parameters for [`PersistAsyncWith::create`].
    type CreateParams;
    /// Parameters for [`PersistAsyncWith::load`].
    type LoadParams;
    /// Error type of [`PersistAsyncWith::create`].
    type CreateError;
    /// Error type of [`PersistAsyncWith::load`].
    type LoadError;
    /// Error type of [`PersistAsyncWith::persist`].
    type PersistError;

    /// Initialize the `Db` and create `Self`.
    fn create(db: &mut Db, params: Self::CreateParams) -> FutureResult<Self, Self::CreateError>;

    /// Initialize the `Db` and load a previously-persisted `Self`.
    fn load(db: &mut Db, params: Self::LoadParams) -> FutureResult<Option<Self>, Self::LoadError>;

    /// Persist changes to the `Db`.
    fn persist<'a>(
        db: &'a mut Db,
        changeset: &'a <Self as Staged>::ChangeSet,
    ) -> FutureResult<'a, (), Self::PersistError>;
}

/// Represents a persisted `T`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Persisted<T> {
    inner: T,
}

impl<T> Persisted<T> {
    /// Create a new persisted `T`.
    pub fn create<Db>(db: &mut Db, params: T::CreateParams) -> Result<Self, T::CreateError>
    where
        T: PersistWith<Db>,
    {
        T::create(db, params).map(|inner| Self { inner })
    }

    /// Create a new persisted `T` with async `Db`.
    pub async fn create_async<Db>(
        db: &mut Db,
        params: T::CreateParams,
    ) -> Result<Self, T::CreateError>
    where
        T: PersistAsyncWith<Db>,
    {
        T::create(db, params).await.map(|inner| Self { inner })
    }

    /// Construct a persisted `T` from `Db`.
    pub fn load<Db>(db: &mut Db, params: T::LoadParams) -> Result<Option<Self>, T::LoadError>
    where
        T: PersistWith<Db>,
    {
        Ok(T::load(db, params)?.map(|inner| Self { inner }))
    }

    /// Construct a persisted `T` from an async `Db`.
    pub async fn load_async<Db>(
        db: &mut Db,
        params: T::LoadParams,
    ) -> Result<Option<Self>, T::LoadError>
    where
        T: PersistAsyncWith<Db>,
    {
        Ok(T::load(db, params).await?.map(|inner| Self { inner }))
    }

    /// Persist staged changes of `T` into `Db`.
    ///
    /// If the database errors, the staged changes will not be cleared.
    pub fn persist<Db>(&mut self, db: &mut Db) -> Result<bool, T::PersistError>
    where
        T: PersistWith<Db>,
    {
        let stage = T::staged(&mut self.inner);
        if stage.is_empty() {
            return Ok(false);
        }
        T::persist(db, &*stage)?;
        stage.take();
        Ok(true)
    }

    /// Persist staged changes of `T` into an async `Db`.
    ///
    /// If the database errors, the staged changes will not be cleared.
    pub async fn persist_async<'a, Db>(
        &'a mut self,
        db: &'a mut Db,
    ) -> Result<bool, T::PersistError>
    where
        T: PersistAsyncWith<Db>,
    {
        let stage = T::staged(&mut self.inner);
        if stage.is_empty() {
            return Ok(false);
        }
        T::persist(db, &*stage).await?;
        stage.take();
        Ok(true)
    }
}

impl<T> Deref for Persisted<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for Persisted<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
