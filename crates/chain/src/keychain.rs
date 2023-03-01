//! Module for keychain based structures.
//!
//! A keychain here is a set of application defined indexes for a minscript descriptor where we can
//! derive script pubkeys at a particular derivation index. The application's index is simply
//! anything that implements `Ord`.
//!
//! [`KeychainTxOutIndex`] indexes script pubkeys of keychains and scans in relevant outpoints (that
//! has a `txout` containing an indexed script pubkey). Internally, this uses [`SpkTxOutIndex`], but
//! also maintains "revealed" and "lookahead" index count per keychain.
//!
//! [`KeychainTracker`] combines [`ChainGraph`] and [`KeychainTxOutIndex`] and enforces atomic
//! changes between both these structures. [`KeychainScan`] is a structure used to update to
//! [`KeychainTracker`] and changes made on a [`KeychainTracker`] are reported by
//! [`KeychainChangeSet`]s.
//!
//! [`SpkTxOutIndex`]: crate::SpkTxOutIndex
use crate::{
    chain_graph::{self, ChainGraph},
    collections::BTreeMap,
    sparse_chain::ChainPosition,
    tx_graph::TxGraph,
    AsTransaction, ForEachTxOut,
};
use bitcoin::Transaction;

#[cfg(feature = "miniscript")]
pub mod persist;
#[cfg(feature = "miniscript")]
pub use persist::*;
#[cfg(feature = "miniscript")]
mod tracker;
#[cfg(feature = "miniscript")]
pub use tracker::*;
#[cfg(feature = "miniscript")]
mod txout_index;
#[cfg(feature = "miniscript")]
pub use txout_index::*;

/// Represents updates to the derivation index of a [`KeychainTxOutIndex`].
///
/// It can be applied to [`KeychainTxOutIndex`] with [`apply_additions`]. [`DerivationAdditions] are
/// monotone in that they will never decrease the revealed derivation index.
///
/// [`KeychainTxOutIndex`]: crate::keychain::KeychainTxOutIndex
/// [`apply_additions`]: crate::keychain::KeychainTxOutIndex::apply_additions
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(
        crate = "serde_crate",
        bound(
            deserialize = "K: Ord + serde::Deserialize<'de>",
            serialize = "K: Ord + serde::Serialize"
        )
    )
)]
#[must_use]
pub struct DerivationAdditions<K>(BTreeMap<K, u32>);

impl<K> DerivationAdditions<K> {
    /// Returns whether the additions are empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get the inner map of keychain to its new derivation index.
    pub fn as_inner(&self) -> &BTreeMap<K, u32> {
        &self.0
    }
}

impl<K: Ord> DerivationAdditions<K> {
    /// Append another [`DerivationAdditions`] into self.
    ///
    /// If keychain already exists, increases the index when other's index > self's index.
    /// If keychain did not exist, append the new keychain.
    pub fn append(&mut self, mut other: Self) {
        self.0.iter_mut().for_each(|(key, index)| {
            if let Some(other_index) = other.0.remove(key) {
                *index = other_index.max(*index);
            }
        });

        self.0.append(&mut other.0);
    }
}

impl<K> Default for DerivationAdditions<K> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<K> AsRef<BTreeMap<K, u32>> for DerivationAdditions<K> {
    fn as_ref(&self) -> &BTreeMap<K, u32> {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
/// An update that includes the last active indexes of each keychain.
pub struct KeychainScan<K, P, T = Transaction> {
    /// The update data in the form of a chain that could be applied
    pub update: ChainGraph<P, T>,
    /// The last active indexes of each keychain
    pub last_active_indices: BTreeMap<K, u32>,
}

impl<K, P, T> Default for KeychainScan<K, P, T> {
    fn default() -> Self {
        Self {
            update: Default::default(),
            last_active_indices: Default::default(),
        }
    }
}

impl<K, P, T> From<ChainGraph<P, T>> for KeychainScan<K, P, T> {
    fn from(update: ChainGraph<P, T>) -> Self {
        KeychainScan {
            update,
            last_active_indices: Default::default(),
        }
    }
}

/// Represents changes to a [`KeychainTracker`].
///
/// This is essentially a combination of [`DerivationAdditions`] and [`chain_graph::ChangeSet`].
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(
        crate = "serde_crate",
        bound(
            deserialize = "K: Ord + serde::Deserialize<'de>, P: serde::Deserialize<'de>, T: Ord + serde::Deserialize<'de>",
            serialize = "K: Ord + serde::Serialize, P: serde::Serialize, T: Ord+ serde::Serialize"
        )
    )
)]
#[must_use]
pub struct KeychainChangeSet<K, P, T = Transaction> {
    /// The changes in local keychain derivation indices
    pub derivation_indices: DerivationAdditions<K>,
    /// The changes that have occurred in the blockchain
    pub chain_graph: chain_graph::ChangeSet<P, T>,
}

impl<K, P, T> Default for KeychainChangeSet<K, P, T> {
    fn default() -> Self {
        Self {
            chain_graph: Default::default(),
            derivation_indices: Default::default(),
        }
    }
}

impl<K, P, T> KeychainChangeSet<K, P, T> {
    /// Returns whether the [`KeychainChangeSet`] is empty (no changes recorded).
    pub fn is_empty(&self) -> bool {
        self.chain_graph.is_empty() && self.derivation_indices.is_empty()
    }

    /// Appends the changes in `other` into `self` such that applying `self` afterwards has the same
    /// effect as sequentially applying the original `self` and `other`.
    ///
    /// Note the derivation indices cannot be decreased so `other` will only change the derivation
    /// index for a keychain if it's entry is higher than the one in `self`.
    pub fn append(&mut self, other: KeychainChangeSet<K, P, T>)
    where
        K: Ord,
        P: ChainPosition,
        T: Ord,
    {
        self.derivation_indices.append(other.derivation_indices);
        self.chain_graph.append(other.chain_graph);
    }
}

impl<K, P, T> From<chain_graph::ChangeSet<P, T>> for KeychainChangeSet<K, P, T> {
    fn from(changeset: chain_graph::ChangeSet<P, T>) -> Self {
        Self {
            chain_graph: changeset,
            ..Default::default()
        }
    }
}

impl<K, P, T> From<DerivationAdditions<K>> for KeychainChangeSet<K, P, T> {
    fn from(additions: DerivationAdditions<K>) -> Self {
        Self {
            derivation_indices: additions,
            ..Default::default()
        }
    }
}

impl<K, P, T> AsRef<TxGraph<T>> for KeychainScan<K, P, T> {
    fn as_ref(&self) -> &TxGraph<T> {
        self.update.graph()
    }
}

impl<K, P, T: AsTransaction> ForEachTxOut for KeychainChangeSet<K, P, T> {
    fn for_each_txout(&self, f: impl FnMut((bitcoin::OutPoint, &bitcoin::TxOut))) {
        self.chain_graph.for_each_txout(f)
    }
}

/// Balance differentiated in various categories.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate",)
)]
pub struct Balance {
    /// All coinbase outputs not yet matured
    pub immature: u64,
    /// Unconfirmed UTXOs generated by a wallet tx
    pub trusted_pending: u64,
    /// Unconfirmed UTXOs received from an external wallet
    pub untrusted_pending: u64,
    /// Confirmed and immediately spendable balance
    pub confirmed: u64,
}

impl Balance {
    /// Get sum of trusted_pending and confirmed coins.
    ///
    /// This is the balance you can spend right now that shouldn't get cancelled via another party
    /// double spending it.
    pub fn trusted_spendable(&self) -> u64 {
        self.confirmed + self.trusted_pending
    }

    /// Get the whole balance visible to the wallet.
    pub fn total(&self) -> u64 {
        self.confirmed + self.trusted_pending + self.untrusted_pending + self.immature
    }
}

impl core::fmt::Display for Balance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{{ immature: {}, trusted_pending: {}, untrusted_pending: {}, confirmed: {} }}",
            self.immature, self.trusted_pending, self.untrusted_pending, self.confirmed
        )
    }
}

impl core::ops::Add for Balance {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            immature: self.immature + other.immature,
            trusted_pending: self.trusted_pending + other.trusted_pending,
            untrusted_pending: self.untrusted_pending + other.untrusted_pending,
            confirmed: self.confirmed + other.confirmed,
        }
    }
}

#[cfg(test)]
mod test {
    use bitcoin::Transaction;

    use crate::TxHeight;

    use super::*;
    #[test]
    fn append_keychain_derivation_indices() {
        #[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
        enum Keychain {
            One,
            Two,
            Three,
            Four,
        }
        let mut lhs_di = BTreeMap::<Keychain, u32>::default();
        let mut rhs_di = BTreeMap::<Keychain, u32>::default();
        lhs_di.insert(Keychain::One, 7);
        lhs_di.insert(Keychain::Two, 0);
        rhs_di.insert(Keychain::One, 3);
        rhs_di.insert(Keychain::Two, 5);
        lhs_di.insert(Keychain::Three, 3);
        rhs_di.insert(Keychain::Four, 4);
        let mut lhs = KeychainChangeSet {
            derivation_indices: DerivationAdditions(lhs_di),
            chain_graph: chain_graph::ChangeSet::<TxHeight, Transaction>::default(),
        };

        let rhs = KeychainChangeSet {
            derivation_indices: DerivationAdditions(rhs_di),
            chain_graph: chain_graph::ChangeSet::<TxHeight, Transaction>::default(),
        };

        lhs.append(rhs);

        // Exiting index doesn't update if new index in `other` is lower than `self`
        assert_eq!(lhs.derivation_indices.0.get(&Keychain::One), Some(&7));
        // Existing index updates if new index in `other` is higher than `self.
        assert_eq!(lhs.derivation_indices.0.get(&Keychain::Two), Some(&5));
        // Existing index unchanged, if keychain doesn't exist in `other`
        assert_eq!(lhs.derivation_indices.0.get(&Keychain::Three), Some(&3));
        // New keychain gets added if keychain is in `other`, but not in `self`.
        assert_eq!(lhs.derivation_indices.0.get(&Keychain::Four), Some(&4));
    }
}
