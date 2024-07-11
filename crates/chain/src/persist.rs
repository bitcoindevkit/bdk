use core::{
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
};

use alloc::boxed::Box;

/// Trait that persists the type with `Db`.
///
/// Methods of this trait should not be called directly.
pub trait PersistWith<Db>: Sized {
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

    /// Create the type and initialize the `Db`.
    fn create(db: &mut Db, params: Self::CreateParams) -> Result<Self, Self::CreateError>;

    /// Load the type from the `Db`.
    fn load(db: &mut Db, params: Self::LoadParams) -> Result<Option<Self>, Self::LoadError>;

    /// Persist staged changes into `Db`.
    fn persist(&mut self, db: &mut Db) -> Result<bool, Self::PersistError>;
}

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Trait that persists the type with an async `Db`.
pub trait PersistAsyncWith<Db>: Sized {
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

    /// Create the type and initialize the `Db`.
    fn create(db: &mut Db, params: Self::CreateParams) -> FutureResult<Self, Self::CreateError>;

    /// Load the type from `Db`.
    fn load(db: &mut Db, params: Self::LoadParams) -> FutureResult<Option<Self>, Self::LoadError>;

    /// Persist staged changes into `Db`.
    fn persist<'a>(&'a mut self, db: &'a mut Db) -> FutureResult<'a, bool, Self::PersistError>;
}

/// Represents a persisted `T`.
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

    /// Contruct a persisted `T` from an async `Db`.
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
    pub fn persist<Db>(&mut self, db: &mut Db) -> Result<bool, T::PersistError>
    where
        T: PersistWith<Db>,
    {
        self.inner.persist(db)
    }

    /// Persist staged changes of `T` into an async `Db`.
    pub async fn persist_async<'a, Db>(
        &'a mut self,
        db: &'a mut Db,
    ) -> Result<bool, T::PersistError>
    where
        T: PersistAsyncWith<Db>,
    {
        self.inner.persist(db).await
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
