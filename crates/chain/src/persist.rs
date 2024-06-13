//! This module is home to the [`PersistBackend`] trait which defines the behavior of a data store
//! required to persist changes made to BDK data structures.
//!
//! The [`CombinedChangeSet`] type encapsulates a combination of [`crate`] structures that are
//! typically persisted together.

#[cfg(feature = "async")]
use alloc::boxed::Box;
#[cfg(feature = "async")]
use async_trait::async_trait;
use core::convert::Infallible;
use core::fmt::{Debug, Display};

/// A changeset containing [`crate`] structures typically persisted together.
#[derive(Debug, Clone, PartialEq)]
#[cfg(feature = "miniscript")]
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
    /// Changes to the [`LocalChain`](crate::local_chain::LocalChain).
    pub chain: crate::local_chain::ChangeSet,
    /// Changes to [`IndexedTxGraph`](crate::indexed_tx_graph::IndexedTxGraph).
    pub indexed_tx_graph: crate::indexed_tx_graph::ChangeSet<A, crate::keychain::ChangeSet<K>>,
    /// Stores the network type of the transaction data.
    pub network: Option<bitcoin::Network>,
}

#[cfg(feature = "miniscript")]
impl<K, A> core::default::Default for CombinedChangeSet<K, A> {
    fn default() -> Self {
        Self {
            chain: core::default::Default::default(),
            indexed_tx_graph: core::default::Default::default(),
            network: None,
        }
    }
}

#[cfg(feature = "miniscript")]
impl<K: Ord, A: crate::Anchor> crate::Append for CombinedChangeSet<K, A> {
    fn append(&mut self, other: Self) {
        crate::Append::append(&mut self.chain, other.chain);
        crate::Append::append(&mut self.indexed_tx_graph, other.indexed_tx_graph);
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

#[cfg(feature = "miniscript")]
impl<K, A> From<crate::local_chain::ChangeSet> for CombinedChangeSet<K, A> {
    fn from(chain: crate::local_chain::ChangeSet) -> Self {
        Self {
            chain,
            ..Default::default()
        }
    }
}

#[cfg(feature = "miniscript")]
impl<K, A> From<crate::indexed_tx_graph::ChangeSet<A, crate::keychain::ChangeSet<K>>>
    for CombinedChangeSet<K, A>
{
    fn from(
        indexed_tx_graph: crate::indexed_tx_graph::ChangeSet<A, crate::keychain::ChangeSet<K>>,
    ) -> Self {
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
pub trait PersistBackend<C> {
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

impl<C> PersistBackend<C> for () {
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
pub trait PersistBackendAsync<C> {
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
impl<C> PersistBackendAsync<C> for () {
    type WriteError = Infallible;
    type LoadError = Infallible;

    async fn write_changes(&mut self, _changeset: &C) -> Result<(), Self::WriteError> {
        Ok(())
    }

    async fn load_changes(&mut self) -> Result<Option<C>, Self::LoadError> {
        Ok(None)
    }
}
