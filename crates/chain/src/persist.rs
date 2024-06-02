//! This module is home to the [`Persist`] trait which defines the behavior of a data store
//! required to persist changes made to BDK data structures.
//!
//! The [`StagedPersist`] type provides a convenient wrapper around implementations of [`Persist`] that
//! allows changes to be staged before committing them.
//!
//! The [`CombinedChangeSet`] type encapsulates a combination of [`crate`] structures that are
//! typically persisted together.

use crate::{indexed_tx_graph, keychain, local_chain, Anchor, Append};
#[cfg(feature = "async")]
use alloc::boxed::Box;
#[cfg(feature = "async")]
use async_trait::async_trait;
use bitcoin::Network;
use core::convert::Infallible;
use core::default::Default;
use core::fmt::{Debug, Display};
use core::mem;

/// A changeset containing [`crate`] structures typically persisted together.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(crate::serde::Deserialize, crate::serde::Serialize),
    serde(
        crate = "crate::serde",
        bound(
            deserialize = "A: Ord + crate::serde::Deserialize<'de>, K: Ord + crate::serde::Deserialize<'de>",
            serialize = "A: Ord + crate::serde::Serialize, K: Ord + crate::serde::Serialize",
        ),
    )
)]
pub struct CombinedChangeSet<K, A> {
    /// Changes to the [`LocalChain`](local_chain::LocalChain).
    pub chain: local_chain::ChangeSet,
    /// Changes to [`IndexedTxGraph`](indexed_tx_graph::IndexedTxGraph).
    pub indexed_tx_graph: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    /// Stores the network type of the transaction data.
    pub network: Option<Network>,
}

impl<K, A> Default for CombinedChangeSet<K, A> {
    fn default() -> Self {
        Self {
            chain: Default::default(),
            indexed_tx_graph: Default::default(),
            network: None,
        }
    }
}

impl<K: Ord, A: Anchor> Append for CombinedChangeSet<K, A> {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.chain, other.chain);
        Append::append(&mut self.indexed_tx_graph, other.indexed_tx_graph);
        if other.network.is_some() {
            debug_assert!(
                self.network.is_none() || self.network == other.network,
                "network type must either be just introduced or remain the same"
            );
            self.network = other.network;
        }
    }

    fn is_empty(&self) -> bool {
        self.chain.is_empty() && self.indexed_tx_graph.is_empty() && self.network.is_none()
    }
}

impl<K, A> From<local_chain::ChangeSet> for CombinedChangeSet<K, A> {
    fn from(chain: local_chain::ChangeSet) -> Self {
        Self {
            chain,
            ..Default::default()
        }
    }
}

impl<K, A> From<indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>>
    for CombinedChangeSet<K, A>
{
    fn from(indexed_tx_graph: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>) -> Self {
        Self {
            indexed_tx_graph,
            ..Default::default()
        }
    }
}

/// A persistence backend for writing and loading changesets.
///
/// `C` represents the changeset; a datatype that records changes made to in-memory data structures
/// that are to be persisted, or retrieved from persistence.
pub trait Persist<C> {
    /// The error the backend returns when it fails to write.
    type WriteError: Debug + Display;

    /// The error the backend returns when it fails to load changesets `C`.
    type LoadError: Debug + Display;

    /// Writes a changeset to the persistence backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it inserts the actual changes into a more structured database. All it needs to guarantee is
    /// that [`load_from_persistence`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_from_persistence`]: Self::load_changes
    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError>;

    /// Return the aggregate changeset `C` from persistence.
    fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError>;
}

impl<C> Persist<C> for () {
    type WriteError = Infallible;
    type LoadError = Infallible;

    fn write_changes(&mut self, _changeset: &C) -> Result<(), Self::WriteError> {
        Ok(())
    }

    fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError> {
        Ok(None)
    }
}

#[cfg(feature = "async")]
/// An async persistence backend for writing and loading changesets.
///
/// `C` represents the changeset; a datatype that records changes made to in-memory data structures
/// that are to be persisted, or retrieved from persistence.
#[async_trait]
pub trait PersistAsync<C> {
    /// The error the backend returns when it fails to write.
    type WriteError: Debug + Display;

    /// The error the backend returns when it fails to load changesets `C`.
    type LoadError: Debug + Display;

    /// Writes a changeset to the persistence backend.
    ///
    /// It is up to the backend what it does with this. It could store every changeset in a list or
    /// it inserts the actual changes into a more structured database. All it needs to guarantee is
    /// that [`load_from_persistence`] restores a keychain tracker to what it should be if all
    /// changesets had been applied sequentially.
    ///
    /// [`load_from_persistence`]: Self::load_changes
    async fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError>;

    /// Return the aggregate changeset `C` from persistence.
    async fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError>;
}

#[cfg(feature = "async")]
#[async_trait]
impl<C> PersistAsync<C> for () {
    type WriteError = Infallible;
    type LoadError = Infallible;

    async fn write_changes(&mut self, _changeset: &C) -> Result<(), Self::WriteError> {
        Ok(())
    }

    async fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError> {
        Ok(None)
    }
}

/// `StagedPersist` adds a convenient staging area for changesets before they are persisted.
///
/// Not all changes to the in-memory representation needs to be written to disk right away, so
/// [`crate::persist::StagedPersist::stage`] can be used to *stage* changes first and then
/// [`crate::persist::StagedPersist::commit`] can be used to write changes to disk.
pub struct StagedPersist<C, P: Persist<C>> {
    inner: P,
    stage: C,
}

impl<C, P: Persist<C>> Persist<C> for StagedPersist<C, P> {
    type WriteError = P::WriteError;
    type LoadError = P::LoadError;

    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError> {
        self.inner.write_changes(changeset)
    }

    fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError> {
        self.inner.load_changes()
    }
}

impl<C, P> StagedPersist<C, P>
where
    C: Default + Append,
    P: Persist<C>,
{
    /// Create a new [`StagedPersist`] adding staging to an inner data store that implements
    /// [`Persist`].
    pub fn new(persist: P) -> Self {
        Self {
            inner: persist,
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

    /// Take the changes that have not been committed yet.
    ///
    /// New staged is set to default;
    pub fn take_staged(&mut self) -> C {
        mem::take(&mut self.stage)
    }

    /// Commit the staged changes to the underlying persistence backend.
    ///
    /// Changes that are committed (if any) are returned.
    ///
    /// # Error
    ///
    /// Returns a backend-defined error if this fails.
    pub fn commit(&mut self) -> Result<Option<C>, P::WriteError> {
        if self.staged().is_empty() {
            return Ok(None);
        }
        let staged = self.take_staged();
        self.write_changes(&staged)
            // if written successfully, take and return `self.stage`
            .map(|_| Some(staged))
    }

    /// Stages a new changeset and commits it (along with any other previously staged changes) to
    /// the persistence backend
    ///
    /// Convenience method for calling [`stage`] and then [`commit`].
    ///
    /// [`stage`]: Self::stage
    /// [`commit`]: Self::commit
    pub fn stage_and_commit(&mut self, changeset: C) -> Result<Option<C>, P::WriteError> {
        self.stage(changeset);
        self.commit()
    }
}

#[cfg(feature = "async")]
/// `StagedPersistAsync` adds a convenient async staging area for changesets before they are persisted.
///
/// Not all changes to the in-memory representation needs to be written to disk right away, so
/// [`StagedPersistAsync::stage`] can be used to *stage* changes first and then
/// [`StagedPersistAsync::commit`] can be used to write changes to disk.
pub struct StagedPersistAsync<C, P: PersistAsync<C>> {
    inner: P,
    staged: C,
}

#[cfg(feature = "async")]
#[async_trait]
impl<C: Send + Sync, P: PersistAsync<C> + Send> PersistAsync<C> for StagedPersistAsync<C, P> {
    type WriteError = P::WriteError;
    type LoadError = P::LoadError;

    async fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError> {
        self.inner.write_changes(changeset).await
    }

    async fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError> {
        self.inner.load_changes().await
    }
}

#[cfg(feature = "async")]
impl<C, P> StagedPersistAsync<C, P>
where
    C: Default + Append + Send + Sync,
    P: PersistAsync<C> + Send,
{
    /// Create a new [`StagedPersistAsync`] adding staging to an inner data store that implements
    /// [`PersistAsync`].
    pub fn new(persist: P) -> Self {
        Self {
            inner: persist,
            staged: Default::default(),
        }
    }

    /// Stage a `changeset` to be committed later with [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn stage(&mut self, changeset: C) {
        self.staged.append(changeset)
    }

    /// Get the changes that have not been committed yet.
    pub fn staged(&self) -> &C {
        &self.staged
    }

    /// Take the changes that have not been committed yet.
    ///
    /// New staged is set to default;
    pub fn take_staged(&mut self) -> C {
        mem::take(&mut self.staged)
    }

    /// Commit the staged changes to the underlying persistence backend.
    ///
    /// Changes that are committed (if any) are returned.
    ///
    /// # Error
    ///
    /// Returns a backend-defined error if this fails.
    pub async fn commit(&mut self) -> Result<Option<C>, P::WriteError> {
        if self.staged().is_empty() {
            return Ok(None);
        }
        let staged = self.take_staged();
        self.write_changes(&staged)
            .await
            // if written successfully, take and return `self.stage`
            .map(|_| Some(staged))
    }

    /// Stages a new changeset and commits it (along with any other previously staged changes) to
    /// the persistence backend
    ///
    /// Convenience method for calling [`stage`] and then [`commit`].
    ///
    /// [`stage`]: Self::stage
    /// [`commit`]: Self::commit
    pub async fn stage_and_commit(&mut self, changeset: C) -> Result<Option<C>, P::WriteError> {
        self.stage(changeset);
        self.commit().await
    }
}

#[cfg(test)]
mod test {
    extern crate core;

    use crate::persist::{Persist, StagedPersist};
    use crate::Append;
    use std::error::Error;
    use std::fmt::{self, Display, Formatter};
    use std::prelude::rust_2015::{String, ToString};
    use TestError::FailedWrite;

    struct TestBackend<C: Default + Append + Clone + ToString> {
        changeset: C,
    }

    #[derive(Debug, Eq, PartialEq)]
    enum TestError {
        FailedWrite,
        FailedLoad,
    }

    impl Display for TestError {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            write!(f, "{:?}", self)
        }
    }

    impl Error for TestError {}

    #[derive(Clone, Default)]
    struct TestChangeSet(Option<String>);

    impl fmt::Display for TestChangeSet {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.clone().0.unwrap_or_default())
        }
    }

    impl Append for TestChangeSet {
        fn append(&mut self, other: Self) {
            if other.0.is_some() {
                self.0 = other.0
            }
        }

        fn is_empty(&self) -> bool {
            self.0.is_none()
        }
    }

    impl<C> Persist<C> for TestBackend<C>
    where
        C: Default + Append + Clone + ToString,
    {
        type WriteError = TestError;
        type LoadError = TestError;

        fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError> {
            if changeset.to_string() == "ERROR" {
                Err(FailedWrite)
            } else {
                self.changeset = changeset.clone();
                Ok(())
            }
        }

        fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError> {
            if self.changeset.to_string() == "ERROR" {
                Err(Self::LoadError::FailedLoad)
            } else {
                Ok(Some(self.changeset.clone()))
            }
        }
    }

    #[test]
    fn test_persist_stage_commit() {
        let backend = TestBackend {
            changeset: TestChangeSet(None),
        };

        let mut staged_backend = StagedPersist::new(backend);
        staged_backend.stage(TestChangeSet(Some("ONE".to_string())));
        staged_backend.stage(TestChangeSet(None));
        staged_backend.stage(TestChangeSet(Some("TWO".to_string())));
        let result = staged_backend.commit();
        assert!(matches!(result, Ok(Some(TestChangeSet(Some(v)))) if v == *"TWO".to_string()));

        let result = staged_backend.commit();
        assert!(matches!(result, Ok(None)));

        staged_backend.stage(TestChangeSet(Some("TWO".to_string())));
        let result = staged_backend.stage_and_commit(TestChangeSet(Some("ONE".to_string())));
        assert!(matches!(result, Ok(Some(TestChangeSet(Some(v)))) if v == *"ONE".to_string()));
    }

    #[test]
    fn test_persist_commit_error() {
        let backend = TestBackend {
            changeset: TestChangeSet(None),
        };
        let mut staged_backend = StagedPersist::new(backend);
        staged_backend.stage(TestChangeSet(Some("ERROR".to_string())));
        let result = staged_backend.commit();
        assert!(matches!(result, Err(e) if e == FailedWrite));
    }
}
