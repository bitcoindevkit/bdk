//! This crate is a collection of core structures for [Bitcoin Dev Kit].

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

mod chain_data;
pub use chain_data::*;

mod checkpoint;
pub use checkpoint::*;

pub mod spk_client;

/// Core structures for [`TxGraph`].
///
/// [`TxGraph`]: https://docs.rs/bdk_chain/latest/bdk_chain/tx_graph/struct.TxGraph.html
pub mod tx_graph {
    use crate::collections::{BTreeMap, BTreeSet, HashMap};
    use alloc::{sync::Arc, vec::Vec};
    use bitcoin::{OutPoint, Transaction, TxOut, Txid};

    /// Data object used to update a [`TxGraph`].
    ///
    /// [`TxGraph`]: https://docs.rs/bdk_chain/latest/bdk_chain/tx_graph/struct.TxGraph.html
    #[derive(Debug, Clone)]
    pub struct Update<A = ()> {
        /// Full transactions.
        pub txs: Vec<Arc<Transaction>>,
        /// Floating txouts.
        pub txouts: BTreeMap<OutPoint, TxOut>,
        /// Transaction anchors.
        pub anchors: BTreeSet<(A, Txid)>,
        /// Seen at times for transactions.
        pub seen_ats: HashMap<Txid, u64>,
    }

    impl<A> Default for Update<A> {
        fn default() -> Self {
            Self {
                txs: Default::default(),
                txouts: Default::default(),
                anchors: Default::default(),
                seen_ats: Default::default(),
            }
        }
    }

    impl<A: Ord> Update<A> {
        /// Extend this update with `other`.
        pub fn extend(&mut self, other: Update<A>) {
            self.txs.extend(other.txs);
            self.txouts.extend(other.txouts);
            self.anchors.extend(other.anchors);
            self.seen_ats.extend(other.seen_ats);
        }
    }
}
