use core::{
    fmt,
    future::Future,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    pin::Pin,
};

use alloc::boxed::Box;
use chain::Merge;

use crate::{descriptor::DescriptorError, ChangeSet, CreateParams, LoadParams, Wallet};

/// Trait that persists [`PersistedWallet`].
///
/// For an async version, use [`AsyncWalletPersister`].
///
/// Associated functions of this trait should not be called directly, and the trait is designed so
/// that associated functions are hard to find (since they are not methods!). [`WalletPersister`] is
/// used by [`PersistedWallet`] (a light wrapper around [`Wallet`]) which enforces some level of
/// safety. Refer to [`PersistedWallet`] for more about the safety checks.
pub trait WalletPersister {
    /// Error type of the persister.
    type Error;

    /// Initialize the `persister` and load all data.
    ///
    /// This is called by [`PersistedWallet::create`] and [`PersistedWallet::load`] to ensure
    /// the [`WalletPersister`] is initialized and returns all data in the `persister`.
    ///
    /// # Implementation Details
    ///
    /// The database schema of the `persister` (if any), should be initialized and migrated here.
    ///
    /// The implementation must return all data currently stored in the `persister`. If there is no
    /// data, return an empty changeset (using [`ChangeSet::default()`]).
    ///
    /// Error should only occur on database failure. Multiple calls to `initialize` should not
    /// error. Calling `initialize` inbetween calls to `persist` should not error.
    ///
    /// Calling [`persist`] before the `persister` is `initialize`d may error. However, some
    /// persister implementations may NOT require initialization at all (and not error).
    ///
    /// [`persist`]: WalletPersister::persist
    fn initialize(persister: &mut Self) -> Result<ChangeSet, Self::Error>;

    /// Persist the given `changeset` to the `persister`.
    ///
    /// This method can fail if the `persister` is not [`initialize`]d.
    ///
    /// [`initialize`]: WalletPersister::initialize
    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<(), Self::Error>;
}

type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Async trait that persists [`PersistedWallet`].
///
/// For a blocking version, use [`WalletPersister`].
///
/// Associated functions of this trait should not be called directly, and the trait is designed so
/// that associated functions are hard to find (since they are not methods!). [`AsyncWalletPersister`] is
/// used by [`PersistedWallet`] (a light wrapper around [`Wallet`]) which enforces some level of
/// safety. Refer to [`PersistedWallet`] for more about the safety checks.
pub trait AsyncWalletPersister {
    /// Error type of the persister.
    type Error;

    /// Initialize the `persister` and load all data.
    ///
    /// This is called by [`PersistedWallet::create_async`] and [`PersistedWallet::load_async`] to
    /// ensure the [`AsyncWalletPersister`] is initialized and returns all data in the `persister`.
    ///
    /// # Implementation Details
    ///
    /// The database schema of the `persister` (if any), should be initialized and migrated here.
    ///
    /// The implementation must return all data currently stored in the `persister`. If there is no
    /// data, return an empty changeset (using [`ChangeSet::default()`]).
    ///
    /// Error should only occur on database failure. Multiple calls to `initialize` should not
    /// error. Calling `initialize` inbetween calls to `persist` should not error.
    ///
    /// Calling [`persist`] before the `persister` is `initialize`d may error. However, some
    /// persister implementations may NOT require initialization at all (and not error).
    ///
    /// [`persist`]: AsyncWalletPersister::persist
    fn initialize<'a>(persister: &'a mut Self) -> FutureResult<'a, ChangeSet, Self::Error>
    where
        Self: 'a;

    /// Persist the given `changeset` to the `persister`.
    ///
    /// This method can fail if the `persister` is not [`initialize`]d.
    ///
    /// [`initialize`]: AsyncWalletPersister::initialize
    fn persist<'a>(
        persister: &'a mut Self,
        changeset: &'a ChangeSet,
    ) -> FutureResult<'a, (), Self::Error>
    where
        Self: 'a;
}

/// Represents a persisted wallet which persists into type `P`.
///
/// This is a light wrapper around [`Wallet`] that enforces some level of safety-checking when used
/// with a [`WalletPersister`] or [`AsyncWalletPersister`] implementation. Safety checks assume that
/// [`WalletPersister`] and/or [`AsyncWalletPersister`] are implemented correctly.
///
/// Checks include:
///
/// * Ensure the persister is initialized before data is persisted.
/// * Ensure there were no previously persisted wallet data before creating a fresh wallet and
///     persisting it.
/// * Only clear the staged changes of [`Wallet`] after persisting succeeds.
/// * Ensure the wallet is persisted to the same `P` type as when created/loaded. Note that this is
///     not completely fool-proof as you can have multiple instances of the same `P` type that are
///     connected to different databases.
#[derive(Debug)]
pub struct PersistedWallet<P> {
    inner: Wallet,
    _marker: PhantomData<fn(&mut P)>,
}

impl<P> Deref for PersistedWallet<P> {
    type Target = Wallet;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> DerefMut for PersistedWallet<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Methods when `P` is a [`WalletPersister`].
impl<P: WalletPersister> PersistedWallet<P> {
    /// Create a new [`PersistedWallet`] with the given `persister` and `params`.
    pub fn create(
        persister: &mut P,
        params: CreateParams,
    ) -> Result<Self, CreateWithPersistError<P::Error>> {
        let existing = P::initialize(persister).map_err(CreateWithPersistError::Persist)?;
        if !existing.is_empty() {
            return Err(CreateWithPersistError::DataAlreadyExists(existing));
        }
        let mut inner =
            Wallet::create_with_params(params).map_err(CreateWithPersistError::Descriptor)?;
        if let Some(changeset) = inner.take_staged() {
            P::persist(persister, &changeset).map_err(CreateWithPersistError::Persist)?;
        }
        Ok(Self {
            inner,
            _marker: PhantomData,
        })
    }

    /// Load a previously [`PersistedWallet`] from the given `persister` and `params`.
    pub fn load(
        persister: &mut P,
        params: LoadParams,
    ) -> Result<Option<Self>, LoadWithPersistError<P::Error>> {
        let changeset = P::initialize(persister).map_err(LoadWithPersistError::Persist)?;
        Wallet::load_with_params(changeset, params)
            .map(|opt| {
                opt.map(|inner| PersistedWallet {
                    inner,
                    _marker: PhantomData,
                })
            })
            .map_err(LoadWithPersistError::InvalidChangeSet)
    }

    /// Persist staged changes of wallet into `persister`.
    ///
    /// Returns whether any new changes were persisted.
    ///
    /// If the `persister` errors, the staged changes will not be cleared.
    pub fn persist(&mut self, persister: &mut P) -> Result<bool, P::Error> {
        match self.inner.staged_mut() {
            Some(stage) => {
                P::persist(persister, &*stage)?;
                let _ = stage.take();
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// Methods when `P` is an [`AsyncWalletPersister`].
impl<P: AsyncWalletPersister> PersistedWallet<P> {
    /// Create a new [`PersistedWallet`] with the given async `persister` and `params`.
    pub async fn create_async(
        persister: &mut P,
        params: CreateParams,
    ) -> Result<Self, CreateWithPersistError<P::Error>> {
        let existing = P::initialize(persister)
            .await
            .map_err(CreateWithPersistError::Persist)?;
        if !existing.is_empty() {
            return Err(CreateWithPersistError::DataAlreadyExists(existing));
        }
        let mut inner =
            Wallet::create_with_params(params).map_err(CreateWithPersistError::Descriptor)?;
        if let Some(changeset) = inner.take_staged() {
            P::persist(persister, &changeset)
                .await
                .map_err(CreateWithPersistError::Persist)?;
        }
        Ok(Self {
            inner,
            _marker: PhantomData,
        })
    }

    /// Load a previously [`PersistedWallet`] from the given async `persister` and `params`.
    pub async fn load_async(
        persister: &mut P,
        params: LoadParams,
    ) -> Result<Option<Self>, LoadWithPersistError<P::Error>> {
        let changeset = P::initialize(persister)
            .await
            .map_err(LoadWithPersistError::Persist)?;
        Wallet::load_with_params(changeset, params)
            .map(|opt| {
                opt.map(|inner| PersistedWallet {
                    inner,
                    _marker: PhantomData,
                })
            })
            .map_err(LoadWithPersistError::InvalidChangeSet)
    }

    /// Persist staged changes of wallet into an async `persister`.
    ///
    /// Returns whether any new changes were persisted.
    ///
    /// If the `persister` errors, the staged changes will not be cleared.
    pub async fn persist_async(&mut self, persister: &mut P) -> Result<bool, P::Error> {
        match self.inner.staged_mut() {
            Some(stage) => {
                P::persist(persister, &*stage).await?;
                let _ = stage.take();
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(feature = "rusqlite")]
impl WalletPersister for bdk_chain::rusqlite::Transaction<'_> {
    type Error = bdk_chain::rusqlite::Error;

    fn initialize(persister: &mut Self) -> Result<ChangeSet, Self::Error> {
        ChangeSet::init_sqlite_tables(&*persister)?;
        ChangeSet::from_sqlite(persister)
    }

    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<(), Self::Error> {
        changeset.persist_to_sqlite(persister)
    }
}

#[cfg(feature = "rusqlite")]
impl WalletPersister for bdk_chain::rusqlite::Connection {
    type Error = bdk_chain::rusqlite::Error;

    fn initialize(persister: &mut Self) -> Result<ChangeSet, Self::Error> {
        let db_tx = persister.transaction()?;
        ChangeSet::init_sqlite_tables(&db_tx)?;
        let changeset = ChangeSet::from_sqlite(&db_tx)?;
        db_tx.commit()?;
        Ok(changeset)
    }

    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<(), Self::Error> {
        let db_tx = persister.transaction()?;
        changeset.persist_to_sqlite(&db_tx)?;
        db_tx.commit()
    }
}

/// Error for [`bdk_file_store`]'s implementation of [`WalletPersister`].
#[cfg(feature = "file_store")]
#[derive(Debug)]
pub enum FileStoreError {
    /// Error when loading from the store.
    Load(bdk_file_store::AggregateChangesetsError<ChangeSet>),
    /// Error when writing to the store.
    Write(std::io::Error),
}

#[cfg(feature = "file_store")]
impl core::fmt::Display for FileStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use core::fmt::Display;
        match self {
            FileStoreError::Load(e) => Display::fmt(e, f),
            FileStoreError::Write(e) => Display::fmt(e, f),
        }
    }
}

#[cfg(feature = "file_store")]
impl std::error::Error for FileStoreError {}

#[cfg(feature = "file_store")]
impl WalletPersister for bdk_file_store::Store<ChangeSet> {
    type Error = FileStoreError;

    fn initialize(persister: &mut Self) -> Result<ChangeSet, Self::Error> {
        persister
            .aggregate_changesets()
            .map(Option::unwrap_or_default)
            .map_err(FileStoreError::Load)
    }

    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<(), Self::Error> {
        persister
            .append_changeset(changeset)
            .map_err(FileStoreError::Write)
    }
}

/// Error type for [`PersistedWallet::load`].
#[derive(Debug, PartialEq)]
pub enum LoadWithPersistError<E> {
    /// Error from persistence.
    Persist(E),
    /// Occurs when the loaded changeset cannot construct [`Wallet`].
    InvalidChangeSet(crate::LoadError),
}

impl<E: fmt::Display> fmt::Display for LoadWithPersistError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persist(err) => fmt::Display::fmt(err, f),
            Self::InvalidChangeSet(err) => fmt::Display::fmt(&err, f),
        }
    }
}

#[cfg(feature = "std")]
impl<E: fmt::Debug + fmt::Display> std::error::Error for LoadWithPersistError<E> {}

/// Error type for [`PersistedWallet::create`].
#[derive(Debug)]
pub enum CreateWithPersistError<E> {
    /// Error from persistence.
    Persist(E),
    /// Persister already has wallet data.
    DataAlreadyExists(ChangeSet),
    /// Occurs when the loaded changeset cannot construct [`Wallet`].
    Descriptor(DescriptorError),
}

impl<E: fmt::Display> fmt::Display for CreateWithPersistError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persist(err) => fmt::Display::fmt(err, f),
            Self::DataAlreadyExists(changeset) => write!(
                f,
                "Cannot create wallet in persister which already contains wallet data: {:?}",
                changeset
            ),
            Self::Descriptor(err) => fmt::Display::fmt(&err, f),
        }
    }
}

#[cfg(feature = "std")]
impl<E: fmt::Debug + fmt::Display> std::error::Error for CreateWithPersistError<E> {}
