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

#[cfg(feature = "miniscript")]
mod txout_index;
#[cfg(feature = "miniscript")]
pub use txout_index::*;

/// Balance, differentiated into various categories.
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
