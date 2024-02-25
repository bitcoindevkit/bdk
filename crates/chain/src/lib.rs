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
pub use bitcoin::absolute::Height;
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
mod persist;
pub use persist::*;
use std::fmt;
use std::ops::Deref;

#[doc(hidden)]
pub mod example_utils;

#[cfg(feature = "miniscript")]
pub use miniscript;
#[cfg(feature = "miniscript")]
mod descriptor_ext;
#[cfg(feature = "miniscript")]
pub use descriptor_ext::DescriptorExt;
#[cfg(feature = "miniscript")]
mod spk_iter;
#[cfg(feature = "miniscript")]
pub use spk_iter::*;

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[cfg(feature = "serde")]
pub extern crate serde_crate as serde;

#[cfg(feature = "bincode")]
extern crate bincode;

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

/// Number of seconds that have elapsed since 00:00:00 UTC on 1 January 1970, the Unix epoch.
#[cfg_attr(feature = "serialize", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
pub struct UnixSeconds(u64);

impl UnixSeconds {
    pub fn from_consensus(n: u64) -> UnixSeconds {
        UnixSeconds(n)
    }
}
impl Default for UnixSeconds {
    fn default() -> Self {
        UnixSeconds(0)
    }
}

#[cfg_attr(feature = "serialize", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Copy, PartialOrd, Ord, core::hash::Hash)]
pub struct BlockHeight(Height);

impl Default for BlockHeight {
    fn default() -> Self {
        return BlockHeight(
            Height::from_consensus(0).expect("Failed to create a default BlockHeight"),
        );
    }
}

impl Deref for BlockHeight {
    type Target = Height;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl BlockHeight {
    pub const ZERO: Self = BlockHeight(Height::ZERO);
    pub const MAX: Self = BlockHeight(Height::MAX);

    pub fn from_consensus(n: u32) -> BlockHeight {
        BlockHeight(Height::from_consensus(n).expect("Invalid height value"))
    }
    pub fn to_consensus_u32(self) -> u32 {
        self.0.to_consensus_u32()
    }
}

impl fmt::Display for BlockHeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_consensus_u32())
    }
}

/// How many confirmations are needed f or a coinbase output to be spent.
pub const COINBASE_MATURITY: u32 = 100;
