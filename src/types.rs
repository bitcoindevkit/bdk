// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use std::convert::AsRef;
use std::ops::Sub;

use bitcoin::blockdata::transaction::{OutPoint, Transaction, TxOut};
use bitcoin::{hash_types::Txid, util::psbt};

use serde::{Deserialize, Serialize};

/// Types of keychains
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeychainKind {
    /// External
    External = 0,
    /// Internal, usually used for change outputs
    Internal = 1,
}

impl KeychainKind {
    /// Return [`KeychainKind`] as a byte
    pub fn as_byte(&self) -> u8 {
        match self {
            KeychainKind::External => b'e',
            KeychainKind::Internal => b'i',
        }
    }
}

impl AsRef<[u8]> for KeychainKind {
    fn as_ref(&self) -> &[u8] {
        match self {
            KeychainKind::External => b"e",
            KeychainKind::Internal => b"i",
        }
    }
}

/// Fee rate
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
// Internally stored as satoshi/vbyte
pub struct FeeRate(f32);

impl FeeRate {
    /// Create a new instance of [`FeeRate`] given a float fee rate in btc/kvbytes
    pub fn from_btc_per_kvb(btc_per_kvb: f32) -> Self {
        FeeRate(btc_per_kvb * 1e5)
    }

    /// Create a new instance of [`FeeRate`] given a float fee rate in satoshi/vbyte
    pub const fn from_sat_per_vb(sat_per_vb: f32) -> Self {
        FeeRate(sat_per_vb)
    }

    /// Create a new [`FeeRate`] with the default min relay fee value
    pub const fn default_min_relay_fee() -> Self {
        FeeRate(1.0)
    }

    /// Return the value as satoshi/vbyte
    pub fn as_sat_vb(&self) -> f32 {
        self.0
    }
}

impl std::default::Default for FeeRate {
    fn default() -> Self {
        FeeRate::default_min_relay_fee()
    }
}

impl Sub for FeeRate {
    type Output = Self;

    fn sub(self, other: FeeRate) -> Self::Output {
        FeeRate(self.0 - other.0)
    }
}

/// An unspent output owned by a [`Wallet`].
///
/// [`Wallet`]: crate::Wallet
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalUtxo {
    /// Reference to a transaction output
    pub outpoint: OutPoint,
    /// Transaction output
    pub txout: TxOut,
    /// Type of keychain
    pub keychain: KeychainKind,
}

/// A [`Utxo`] with its `satisfaction_weight`.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightedUtxo {
    /// The weight of the witness data and `scriptSig` expressed in [weight units]. This is used to
    /// properly maintain the feerate when adding this input to a transaction during coin selection.
    ///
    /// [weight units]: https://en.bitcoin.it/wiki/Weight_units
    pub satisfaction_weight: usize,
    /// The UTXO
    pub utxo: Utxo,
}

#[derive(Debug, Clone, PartialEq)]
/// An unspent transaction output (UTXO).
pub enum Utxo {
    /// A UTXO owned by the local wallet.
    Local(LocalUtxo),
    /// A UTXO owned by another wallet.
    Foreign {
        /// The location of the output.
        outpoint: OutPoint,
        /// The information about the input we require to add it to a PSBT.
        // Box it to stop the type being too big.
        psbt_input: Box<psbt::Input>,
    },
}

impl Utxo {
    /// Get the location of the UTXO
    pub fn outpoint(&self) -> OutPoint {
        match &self {
            Utxo::Local(local) => local.outpoint,
            Utxo::Foreign { outpoint, .. } => *outpoint,
        }
    }

    /// Get the `TxOut` of the UTXO
    pub fn txout(&self) -> &TxOut {
        match &self {
            Utxo::Local(local) => &local.txout,
            Utxo::Foreign {
                outpoint,
                psbt_input,
            } => {
                if let Some(prev_tx) = &psbt_input.non_witness_utxo {
                    return &prev_tx.output[outpoint.vout as usize];
                }

                if let Some(txout) = &psbt_input.witness_utxo {
                    return txout;
                }

                unreachable!("Foreign UTXOs will always have one of these set")
            }
        }
    }
}

/// A wallet transaction
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct TransactionDetails {
    /// Optional transaction
    pub transaction: Option<Transaction>,
    /// Transaction id
    pub txid: Txid,

    /// Received value (sats)
    pub received: u64,
    /// Sent value (sats)
    pub sent: u64,
    /// Fee value (sats) if available
    pub fee: Option<u64>,
    /// If the transaction is confirmed, contains height and timestamp of the block containing the
    /// transaction, unconfirmed transaction contains `None`.
    pub confirmation_time: Option<ConfirmationTime>,
    /// Whether the tx has been verified against the consensus rules
    ///
    /// Confirmed txs are considered "verified" by default, while unconfirmed txs are checked to
    /// ensure an unstrusted [`Blockchain`](crate::blockchain::Blockchain) backend can't trick the
    /// wallet into using an invalid tx as an RBF template.
    ///
    /// The check is only perfomed when the `verify` feature is enabled.
    #[serde(default = "bool::default")] // default to `false` if not specified
    pub verified: bool,
}

/// Block height and timestamp of the block containing the confirmed transaction
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfirmationTime {
    /// confirmation block height
    pub height: u32,
    /// confirmation block timestamp
    pub timestamp: u64,
}

impl ConfirmationTime {
    /// Returns `Some` `ConfirmationTime` if both `height` and `timestamp` are `Some`
    pub fn new(height: Option<u32>, timestamp: Option<u64>) -> Option<Self> {
        match (height, timestamp) {
            (Some(height), Some(timestamp)) => Some(ConfirmationTime { height, timestamp }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_store_feerate_in_const() {
        const _MY_RATE: FeeRate = FeeRate::from_sat_per_vb(10.0);
        const _MIN_RELAY: FeeRate = FeeRate::default_min_relay_fee();
    }
}
