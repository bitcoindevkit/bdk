use core::fmt;

use crate::{descriptor::DescriptorError, Wallet};

/// Represents a persisted wallet.
pub type PersistedWallet<K> = bdk_chain::Persisted<Wallet<K>>;

#[cfg(feature = "rusqlite")]
impl<'c, K> chain::PersistWith<bdk_chain::rusqlite::Transaction<'c>> for Wallet<K>
where
    K: core::fmt::Debug
        + Clone
        + Ord
        + Send
        + Sync
        + serde::Serialize
        + serde::de::DeserializeOwned,
{
    type CreateParams = crate::CreateParams<K>;
    type LoadParams = crate::LoadParams<K>;

    type CreateError = CreateWithPersistError<bdk_chain::rusqlite::Error>;
    type LoadError = LoadWithPersistError<K, bdk_chain::rusqlite::Error>;
    type PersistError = bdk_chain::rusqlite::Error;

    fn create(
        db: &mut bdk_chain::rusqlite::Transaction<'c>,
        params: Self::CreateParams,
    ) -> Result<Self, Self::CreateError> {
        let mut wallet =
            Self::create_with_params(params).map_err(CreateWithPersistError::Descriptor)?;
        if let Some(changeset) = wallet.take_staged() {
            changeset
                .persist_to_sqlite(db)
                .map_err(CreateWithPersistError::Persist)?;
        }
        Ok(wallet)
    }

    fn load(
        conn: &mut bdk_chain::rusqlite::Transaction<'c>,
        params: Self::LoadParams,
    ) -> Result<Option<Self>, Self::LoadError> {
        let changeset =
            crate::ChangeSet::from_sqlite(conn).map_err(LoadWithPersistError::Persist)?;
        if chain::Merge::is_empty(&changeset) {
            return Ok(None);
        }
        Self::load_with_params(changeset, params).map_err(LoadWithPersistError::InvalidChangeSet)
    }

    fn persist(
        db: &mut bdk_chain::rusqlite::Transaction<'c>,
        changeset: &<Self as chain::Staged>::ChangeSet,
    ) -> Result<(), Self::PersistError> {
        changeset.persist_to_sqlite(db)
    }
}

#[cfg(feature = "rusqlite")]
impl<K> chain::PersistWith<bdk_chain::rusqlite::Connection> for Wallet<K>
where
    K: core::fmt::Debug
        + Clone
        + Ord
        + Send
        + Sync
        + serde::Serialize
        + serde::de::DeserializeOwned,
{
    type CreateParams = crate::CreateParams<K>;
    type LoadParams = crate::LoadParams<K>;

    type CreateError = CreateWithPersistError<bdk_chain::rusqlite::Error>;
    type LoadError = LoadWithPersistError<K, bdk_chain::rusqlite::Error>;
    type PersistError = bdk_chain::rusqlite::Error;

    fn create(
        db: &mut bdk_chain::rusqlite::Connection,
        params: Self::CreateParams,
    ) -> Result<Self, Self::CreateError> {
        let mut db_tx = db.transaction().map_err(CreateWithPersistError::Persist)?;
        let wallet = chain::PersistWith::create(&mut db_tx, params)?;
        db_tx.commit().map_err(CreateWithPersistError::Persist)?;
        Ok(wallet)
    }

    fn load(
        db: &mut bdk_chain::rusqlite::Connection,
        params: Self::LoadParams,
    ) -> Result<Option<Self>, Self::LoadError> {
        let mut db_tx = db.transaction().map_err(LoadWithPersistError::Persist)?;
        let wallet_opt = chain::PersistWith::load(&mut db_tx, params)?;
        db_tx.commit().map_err(LoadWithPersistError::Persist)?;
        Ok(wallet_opt)
    }

    fn persist(
        db: &mut bdk_chain::rusqlite::Connection,
        changeset: &<Self as chain::Staged>::ChangeSet,
    ) -> Result<(), Self::PersistError> {
        let db_tx = db.transaction()?;
        changeset.persist_to_sqlite(&db_tx)?;
        db_tx.commit()
    }
}

#[cfg(feature = "file_store")]
impl<K> chain::PersistWith<bdk_file_store::Store<crate::ChangeSet<K>>> for Wallet<K>
where
    K: core::fmt::Debug
        + Clone
        + Ord
        + Send
        + Sync
        + serde::Serialize
        + serde::de::DeserializeOwned,
{
    type CreateParams = crate::CreateParams<K>;
    type LoadParams = crate::LoadParams<K>;
    type CreateError = CreateWithPersistError<std::io::Error>;
    type LoadError =
        LoadWithPersistError<K, bdk_file_store::AggregateChangesetsError<crate::ChangeSet<K>>>;
    type PersistError = std::io::Error;

    fn create(
        db: &mut bdk_file_store::Store<crate::ChangeSet<K>>,
        params: Self::CreateParams,
    ) -> Result<Self, Self::CreateError> {
        let mut wallet =
            Self::create_with_params(params).map_err(CreateWithPersistError::Descriptor)?;
        if let Some(changeset) = wallet.take_staged() {
            db.append_changeset(&changeset)
                .map_err(CreateWithPersistError::Persist)?;
        }
        Ok(wallet)
    }

    fn load(
        db: &mut bdk_file_store::Store<crate::ChangeSet<K>>,
        params: Self::LoadParams,
    ) -> Result<Option<Self>, Self::LoadError> {
        let changeset = db
            .aggregate_changesets()
            .map_err(LoadWithPersistError::Persist)?
            .unwrap_or_default();
        Self::load_with_params(changeset, params).map_err(LoadWithPersistError::InvalidChangeSet)
    }

    fn persist(
        db: &mut bdk_file_store::Store<crate::ChangeSet<K>>,
        changeset: &<Self as chain::Staged>::ChangeSet,
    ) -> Result<(), Self::PersistError> {
        db.append_changeset(changeset)
    }
}

/// Error type for [`PersistedWallet::load`].
#[derive(Debug, PartialEq)]
pub enum LoadWithPersistError<K, E> {
    /// Error from persistence.
    Persist(E),
    /// Occurs when the loaded changeset cannot construct [`Wallet`].
    InvalidChangeSet(crate::LoadError<K>),
}

impl<K: fmt::Debug, E: fmt::Display> fmt::Display for LoadWithPersistError<K, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persist(err) => fmt::Display::fmt(err, f),
            Self::InvalidChangeSet(err) => fmt::Display::fmt(&err, f),
        }
    }
}

#[cfg(feature = "std")]
impl<K: fmt::Debug, E: fmt::Debug + fmt::Display> std::error::Error for LoadWithPersistError<K, E> {}

/// Error type for [`PersistedWallet::create`].
#[derive(Debug)]
pub enum CreateWithPersistError<E> {
    /// Error from persistence.
    Persist(E),
    /// Occurs when the loaded changeset cannot construct [`Wallet`].
    Descriptor(DescriptorError),
}

impl<E: fmt::Display> fmt::Display for CreateWithPersistError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Persist(err) => fmt::Display::fmt(err, f),
            Self::Descriptor(err) => fmt::Display::fmt(&err, f),
        }
    }
}

#[cfg(feature = "std")]
impl<E: fmt::Debug + fmt::Display> std::error::Error for CreateWithPersistError<E> {}
