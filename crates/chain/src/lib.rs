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
//!    you do it synchronously or asynchronously. If you know a fact about the blockchain, you can just
//!    tell `bdk_chain`'s APIs about it, and that information will be integrated, if it can be done
//!    consistently.
//! 2. Data persistence agnostic -- `bdk_chain` does not care where you cache on-chain data, what you
//!    cache or how you retrieve it from persistent storage.
//!
//! [Bitcoin Dev Kit]: https://bitcoindevkit.org/

#![no_std]
#![warn(missing_docs)]

pub use bitcoin;
mod spk_txout_index;
pub use spk_txout_index::*;
mod chain_data;
pub use chain_data::*;
pub mod indexed_tx_graph;
pub use indexed_tx_graph::IndexedTxGraph;
pub mod keychain;
pub mod local_chain;
mod tx_data_traits;
pub mod tx_graph;
pub use tx_data_traits::*;
pub use tx_graph::TxGraph;
mod chain_oracle;
pub use chain_oracle::*;

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
pub use spk_iter::*;
pub mod spk_client;

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[cfg(feature = "serde")]
pub extern crate serde_crate as serde;

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

#[cfg(all(not(feature = "std"), feature = "hashbrown"))]
extern crate hashbrown;

// When no-std use `alloc`'s Hash collections. This is activated by default
#[cfg(all(not(feature = "std"), not(feature = "hashbrown")))]
#[doc(hidden)]
pub mod collections {
    #![allow(dead_code)]
    pub type HashSet<K> = alloc::collections::BTreeSet<K>;
    pub type HashMap<K, V> = alloc::collections::BTreeMap<K, V>;
    pub use alloc::collections::{btree_map as hash_map, *};
}

// When we have std, use `std`'s all collections
#[cfg(all(feature = "std", not(feature = "hashbrown")))]
#[doc(hidden)]
pub mod collections {
    pub use std::collections::{hash_map, *};
}

// With this special feature `hashbrown`, use `hashbrown`'s hash collections, and else from `alloc`.
#[cfg(feature = "hashbrown")]
#[doc(hidden)]
pub mod collections {
    #![allow(dead_code)]
    pub type HashSet<K> = hashbrown::HashSet<K>;
    pub type HashMap<K, V> = hashbrown::HashMap<K, V>;
    pub use alloc::collections::*;
    pub use hashbrown::hash_map;
}

/// How many confirmations are needed f or a coinbase output to be spent.
pub const COINBASE_MATURITY: u32 = 100;
