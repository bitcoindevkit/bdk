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
    /// Create a new instance checking the value provided
    ///
    /// ## Panics
    ///
    /// Panics if the value is not [normal](https://doc.rust-lang.org/std/primitive.f32.html#method.is_normal) (except if it's a positive zero) or negative.
    fn new_checked(value: f32) -> Self {
        assert!(value.is_normal() || value == 0.0);
        assert!(value.is_sign_positive());

        FeeRate(value)
    }

    /// Create a new instance of [`FeeRate`] given a float fee rate in sats/kwu
    pub fn from_sat_per_kwu(sat_per_kwu: f32) -> Self {
        FeeRate::new_checked(sat_per_kwu / 250.0_f32)
    }

    /// Create a new instance of [`FeeRate`] given a float fee rate in sats/kvb
    pub fn from_sat_per_kvb(sat_per_kvb: f32) -> Self {
        FeeRate::new_checked(sat_per_kvb / 1000.0_f32)
    }

    /// Create a new instance of [`FeeRate`] given a float fee rate in btc/kvbytes
    ///
    /// ## Panics
    ///
    /// Panics if the value is not [normal](https://doc.rust-lang.org/std/primitive.f32.html#method.is_normal) (except if it's a positive zero) or negative.
    pub fn from_btc_per_kvb(btc_per_kvb: f32) -> Self {
        FeeRate::new_checked(btc_per_kvb * 1e5)
    }

    /// Create a new instance of [`FeeRate`] given a float fee rate in satoshi/vbyte
    ///
    /// ## Panics
    ///
    /// Panics if the value is not [normal](https://doc.rust-lang.org/std/primitive.f32.html#method.is_normal) (except if it's a positive zero) or negative.
    pub fn from_sat_per_vb(sat_per_vb: f32) -> Self {
        FeeRate::new_checked(sat_per_vb)
    }

    /// Create a new [`FeeRate`] with the default min relay fee value
    pub const fn default_min_relay_fee() -> Self {
        FeeRate(1.0)
    }

    /// Calculate fee rate from `fee` and weight units (`wu`).
    pub fn from_wu(fee: u64, wu: usize) -> FeeRate {
        Self::from_vb(fee, wu.vbytes())
    }

    /// Calculate fee rate from `fee` and `vbytes`.
    pub fn from_vb(fee: u64, vbytes: usize) -> FeeRate {
        let rate = fee as f32 / vbytes as f32;
        Self::from_sat_per_vb(rate)
    }

    /// Return the value as satoshi/vbyte
    pub fn as_sat_per_vb(&self) -> f32 {
        self.0
    }

    /// Calculate absolute fee in Satoshis using size in weight units.
    pub fn fee_wu(&self, wu: usize) -> u64 {
        self.fee_vb(wu.vbytes())
    }

    /// Calculate absolute fee in Satoshis using size in virtual bytes.
    pub fn fee_vb(&self, vbytes: usize) -> u64 {
        (self.as_sat_per_vb() * vbytes as f32).ceil() as u64
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

/// Trait implemented by types that can be used to measure weight units.
pub trait Vbytes {
    /// Convert weight units to virtual bytes.
    fn vbytes(self) -> usize;
}

impl Vbytes for usize {
    fn vbytes(self) -> usize {
        // ref: https://github.com/bitcoin/bips/blob/master/bip-0141.mediawiki#transaction-size-calculations
        (self as f32 / 4.0).ceil() as usize
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
    /// Whether this UTXO is spent or not
    pub is_spent: bool,
}

/// A [`Utxo`] with its `satisfaction_weight`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedUtxo {
    /// The weight of the witness data and `scriptSig` expressed in [weight units]. This is used to
    /// properly maintain the feerate when adding this input to a transaction during coin selection.
    ///
    /// [weight units]: https://en.bitcoin.it/wiki/Weight_units
    pub satisfaction_weight: usize,
    /// The UTXO
    pub utxo: Utxo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TransactionDetails {
    /// Optional transaction
    pub transaction: Option<Transaction>,
    /// Transaction id
    pub txid: Txid,
    /// Received value (sats)
    /// Sum of owned outputs of this transaction.
    pub received: u64,
    /// Sent value (sats)
    /// Sum of owned inputs of this transaction.
    pub sent: u64,
    /// Fee value (sats) if confirmed.
    /// The availability of the fee depends on the backend. It's never `None` with an Electrum
    /// Server backend, but it could be `None` with a Bitcoin RPC node without txindex that receive
    /// funds while offline.
    pub fee: Option<u64>,
    /// If the transaction is confirmed, contains height and Unix timestamp of the block containing the
    /// transaction, unconfirmed transaction contains `None`.
    pub confirmation_time: Option<BlockTime>,
}

impl PartialOrd for TransactionDetails {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TransactionDetails {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.confirmation_time
            .cmp(&other.confirmation_time)
            .then_with(|| self.txid.cmp(&other.txid))
    }
}

/// Block height and timestamp of a block
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockTime {
    /// confirmation block height
    pub height: u32,
    /// confirmation block timestamp
    pub timestamp: u64,
}

impl PartialOrd for BlockTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BlockTime {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.height
            .cmp(&other.height)
            .then_with(|| self.timestamp.cmp(&other.timestamp))
    }
}

/// **DEPRECATED**: Confirmation time of a transaction
///
/// The structure has been renamed to `BlockTime`
#[deprecated(note = "This structure has been renamed to `BlockTime`")]
pub type ConfirmationTime = BlockTime;

impl BlockTime {
    /// Returns `Some` `BlockTime` if both `height` and `timestamp` are `Some`
    pub fn new(height: Option<u32>, timestamp: Option<u64>) -> Option<Self> {
        match (height, timestamp) {
            (Some(height), Some(timestamp)) => Some(BlockTime { height, timestamp }),
            _ => None,
        }
    }
}

/// Balance differentiated in various categories
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Default)]
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
    /// Get sum of trusted_pending and confirmed coins
    pub fn get_spendable(&self) -> u64 {
        self.confirmed + self.trusted_pending
    }

    /// Get the whole balance visible to the wallet
    pub fn get_total(&self) -> u64 {
        self.confirmed + self.trusted_pending + self.untrusted_pending + self.immature
    }
}

impl std::fmt::Display for Balance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ immature: {}, trusted_pending: {}, untrusted_pending: {}, confirmed: {} }}",
            self.immature, self.trusted_pending, self.untrusted_pending, self.confirmed
        )
    }
}

impl std::ops::Add for Balance {
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

impl std::iter::Sum for Balance {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(
            Balance {
                ..Default::default()
            },
            |a, b| a + b,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn sort_block_time() {
        let block_time_a = BlockTime {
            height: 100,
            timestamp: 100,
        };

        let block_time_b = BlockTime {
            height: 100,
            timestamp: 110,
        };

        let block_time_c = BlockTime {
            height: 0,
            timestamp: 0,
        };

        let mut vec = vec![
            block_time_a.clone(),
            block_time_b.clone(),
            block_time_c.clone(),
        ];
        vec.sort();
        let expected = vec![block_time_c, block_time_a, block_time_b];

        assert_eq!(vec, expected)
    }

    #[test]
    fn sort_tx_details() {
        let block_time_a = BlockTime {
            height: 100,
            timestamp: 100,
        };

        let block_time_b = BlockTime {
            height: 0,
            timestamp: 0,
        };

        let tx_details_a = TransactionDetails {
            transaction: None,
            txid: Txid::from_inner([0; 32]),
            received: 0,
            sent: 0,
            fee: None,
            confirmation_time: None,
        };

        let tx_details_b = TransactionDetails {
            transaction: None,
            txid: Txid::from_inner([0; 32]),
            received: 0,
            sent: 0,
            fee: None,
            confirmation_time: Some(block_time_a),
        };

        let tx_details_c = TransactionDetails {
            transaction: None,
            txid: Txid::from_inner([0; 32]),
            received: 0,
            sent: 0,
            fee: None,
            confirmation_time: Some(block_time_b.clone()),
        };

        let tx_details_d = TransactionDetails {
            transaction: None,
            txid: Txid::from_inner([1; 32]),
            received: 0,
            sent: 0,
            fee: None,
            confirmation_time: Some(block_time_b),
        };

        let mut vec = vec![
            tx_details_a.clone(),
            tx_details_b.clone(),
            tx_details_c.clone(),
            tx_details_d.clone(),
        ];
        vec.sort();
        let expected = vec![tx_details_a, tx_details_c, tx_details_d, tx_details_b];

        assert_eq!(vec, expected)
    }

    #[test]
    fn can_store_feerate_in_const() {
        const _MIN_RELAY: FeeRate = FeeRate::default_min_relay_fee();
    }

    #[test]
    #[should_panic]
    fn test_invalid_feerate_neg_zero() {
        let _ = FeeRate::from_sat_per_vb(-0.0);
    }

    #[test]
    #[should_panic]
    fn test_invalid_feerate_neg_value() {
        let _ = FeeRate::from_sat_per_vb(-5.0);
    }

    #[test]
    #[should_panic]
    fn test_invalid_feerate_nan() {
        let _ = FeeRate::from_sat_per_vb(f32::NAN);
    }

    #[test]
    #[should_panic]
    fn test_invalid_feerate_inf() {
        let _ = FeeRate::from_sat_per_vb(f32::INFINITY);
    }

    #[test]
    fn test_valid_feerate_pos_zero() {
        let _ = FeeRate::from_sat_per_vb(0.0);
    }

    #[test]
    fn test_fee_from_btc_per_kvb() {
        let fee = FeeRate::from_btc_per_kvb(1e-5);
        assert!((fee.as_sat_per_vb() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fee_from_sat_per_vbyte() {
        let fee = FeeRate::from_sat_per_vb(1.0);
        assert!((fee.as_sat_per_vb() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fee_default_min_relay_fee() {
        let fee = FeeRate::default_min_relay_fee();
        assert!((fee.as_sat_per_vb() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fee_from_sat_per_kvb() {
        let fee = FeeRate::from_sat_per_kvb(1000.0);
        assert!((fee.as_sat_per_vb() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fee_from_sat_per_kwu() {
        let fee = FeeRate::from_sat_per_kwu(250.0);
        assert!((fee.as_sat_per_vb() - 1.0).abs() < f32::EPSILON);
    }
}
