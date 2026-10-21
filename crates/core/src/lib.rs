//! This crate is a collection of core structures for [Bitcoin Dev Kit].
//!
//! # Features
//!
//! - `std` (default): use the standard library, including its `HashMap`/`HashSet`. Without it, this
//!   crate is `no_std` and uses `alloc`'s `BTreeMap`/`BTreeSet` instead.
//! - `serde`: implement `serde::Serialize`/`serde::Deserialize` for this crate's types.
//! - `hashbrown`: use [`hashbrown`](https://docs.rs/hashbrown)'s `HashMap`/`HashSet` for this
//!   crate's hash collections (e.g. `bdk_chain`'s `TxGraph`), on `std` builds too. These hash with
//!   `ahash` using fixed, publicly known keys unless `ahash`'s `runtime-rng` or `compile-time-rng`
//!   feature is enabled somewhere in the dependency graph. With fixed keys, a malicious chain
//!   source can send transactions crafted to collide, making insertion quadratic. To randomize the
//!   keys, depend on `ahash = { version = "0.8", features = ["runtime-rng"] }` (or
//!   `compile-time-rng`) yourself.

// only enables the `doc_cfg` feature when the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    docsrs,
    doc(html_logo_url = "https://github.com/bitcoindevkit/bdk/raw/master/static/bdk.png")
)]
#![no_std]
#![warn(missing_docs)]

pub use bitcoin;

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[allow(unused_imports)]
#[cfg(feature = "std")]
#[macro_use]
extern crate std;

#[cfg(feature = "serde")]
pub extern crate serde;

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

/// A tuple of keychain index and `T` representing the indexed value.
pub type Indexed<T> = (u32, T);
/// A tuple of keychain `K`, derivation index (`u32`) and a `T` associated with them.
pub type KeychainIndexed<K, T> = ((K, u32), T);

mod block_id;
pub use block_id::*;

mod checkpoint;
pub use checkpoint::*;

mod checkpoint_entry;
pub use checkpoint_entry::*;

mod tx_update;
pub use tx_update::*;

mod merge;
pub use merge::*;

pub mod spk_client;

mod chain_task;
pub use chain_task::*;

mod block_queries;
pub use block_queries::*;
