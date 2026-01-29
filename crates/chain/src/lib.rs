//! This crate is a collection of core structures for [Bitcoin Dev Kit].
//!
//! The goal of this crate is to give wallets the mechanisms needed to:
//!
//! 1. Figure out what data they need to fetch.
//! 2. Process the data in a way that never leads to inconsistent states.
//! 3. Fully index that data and expose it to be consumed without friction.
//!
//! Our design goals for these mechanisms are:
//!
//! 1. Data source agnostic -- nothing in `bdk_chain` cares about where you get data from or whether
//!    you do it synchronously or asynchronously. If you know a fact about the blockchain, you can
//!    just tell `bdk_chain`'s APIs about it, and that information will be integrated, if it can be
//!    done consistently.
//! 2. Data persistence agnostic -- `bdk_chain` does not care where you cache on-chain data, what
//!    you cache or how you retrieve it from persistent storage.
//!
//! [Bitcoin Dev Kit]: https://bitcoindevkit.org/

// only enables the `doc_cfg` feature when the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    docsrs,
    doc(html_logo_url = "https://github.com/bitcoindevkit/bdk/raw/master/static/bdk.png")
)]
#![no_std]
#![warn(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub use bitcoin;
mod balance;
pub use balance::*;
mod chain_data;
pub use chain_data::*;
pub mod indexed_tx_graph;
pub use indexed_tx_graph::IndexedTxGraph;
pub mod indexer;
pub use indexer::spk_txout;
pub use indexer::Indexer;
pub mod local_chain;
mod tx_data_traits;
pub use tx_data_traits::*;
pub mod tx_graph;
pub use tx_graph::TxGraph;
mod chain_oracle;
pub use chain_oracle::*;
mod canonical_iter;
pub use canonical_iter::*;

#[doc(hidden)]
pub mod example_utils;

#[cfg(feature = "miniscript")]
pub use miniscript;
#[cfg(feature = "miniscript")]
mod descriptor_ext;
#[cfg(feature = "miniscript")]
pub use descriptor_ext::{DescriptorExt, DescriptorId};
#[cfg(feature = "miniscript")]
mod spk_iter;
#[cfg(feature = "miniscript")]
pub use indexer::keychain_txout;
#[cfg(feature = "miniscript")]
pub use spk_iter::*;
#[cfg(feature = "rusqlite")]
pub mod rusqlite_impl;

pub extern crate bdk_core;
pub use bdk_core::*;

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;
#[cfg(feature = "rusqlite")]
pub extern crate rusqlite;
#[cfg(feature = "serde")]
pub extern crate serde;

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

/// A wrapper that we use to impl remote traits for types in our crate or dependency crates.
pub struct Impl<T>(pub T);

impl<T> Impl<T> {
    /// Returns the inner `T`.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Impl<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> core::ops::Deref for Impl<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
