//! Module for keychain related structures.
//!
//! A keychain here is a set of application-defined indexes for a miniscript descriptor where we can
//! derive script pubkeys at a particular derivation index. The application's index is simply
//! anything that implements `Ord`.
//!
//! [`KeychainTxOutIndex`] indexes script pubkeys of keychains and scans in relevant outpoints (that
//! has a `txout` containing an indexed script pubkey). Internally, this uses [`SpkTxOutIndex`], but
//! also maintains "revealed" and "lookahead" index counts per keychain.
//!
//! [`SpkTxOutIndex`]: crate::SpkTxOutIndex

use crate::{collections::BTreeMap, Append};

#[cfg(feature = "miniscript")]
mod txout_index;
#[cfg(feature = "miniscript")]
pub use txout_index::*;

/// Represents updates to the derivation index of a [`KeychainTxOutIndex`].
/// It maps each keychain `K` to its last revealed index.
///
/// It can be applied to [`KeychainTxOutIndex`] with [`apply_changeset`]. [`ChangeSet] are
/// monotone in that they will never decrease the revealed derivation index.
///
/// [`KeychainTxOutIndex`]: crate::keychain::KeychainTxOutIndex
/// [`apply_changeset`]: crate::keychain::KeychainTxOutIndex::apply_changeset
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
pub struct ChangeSet<K>(pub BTreeMap<K, u32>);

impl<K> ChangeSet<K> {
    /// Get the inner map of the keychain to its new derivation index.
    pub fn as_inner(&self) -> &BTreeMap<K, u32> {
        &self.0
    }
}

impl<K: Ord> Append for ChangeSet<K> {
    /// Append another [`ChangeSet`] into self.
    ///
    /// If the keychain already exists, increase the index when the other's index > self's index.
    /// If the keychain did not exist, append the new keychain.
    fn append(&mut self, mut other: Self) {
        self.0.iter_mut().for_each(|(key, index)| {
            if let Some(other_index) = other.0.remove(key) {
                *index = other_index.max(*index);
            }
        });

        self.0.append(&mut other.0);
    }

    /// Returns whether the changeset are empty.
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<K> Default for ChangeSet<K> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<K> AsRef<BTreeMap<K, u32>> for ChangeSet<K> {
    fn as_ref(&self) -> &BTreeMap<K, u32> {
        &self.0
    }
}

#[cfg(test)]
mod test {
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

        let mut lhs = ChangeSet(lhs_di);
        let rhs = ChangeSet(rhs_di);
        lhs.append(rhs);

        // Exiting index doesn't update if the new index in `other` is lower than `self`.
        assert_eq!(lhs.0.get(&Keychain::One), Some(&7));
        // Existing index updates if the new index in `other` is higher than `self`.
        assert_eq!(lhs.0.get(&Keychain::Two), Some(&5));
        // Existing index is unchanged if keychain doesn't exist in `other`.
        assert_eq!(lhs.0.get(&Keychain::Three), Some(&3));
        // New keychain gets added if the keychain is in `other` but not in `self`.
        assert_eq!(lhs.0.get(&Keychain::Four), Some(&4));
    }
}
