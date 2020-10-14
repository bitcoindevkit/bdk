// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Coin selection
//!
//! This module provides the trait [`CoinSelectionAlgorithm`] that can be implemented to
//! define custom coin selection algorithms.
//!
//! The coin selection algorithm is not globally part of a [`Wallet`](super::Wallet), instead it
//! is selected whenever a [`Wallet::create_tx`](super::Wallet::create_tx) call is made, through
//! the use of the [`TxBuilder`] structure, specifically with
//! [`TxBuilder::coin_selection`](super::tx_builder::TxBuilder::coin_selection) method.
//!
//! The [`DefaultCoinSelectionAlgorithm`] selects the default coin selection algorithm that
//! [`TxBuilder`] uses, if it's not explicitly overridden.
//!
//! [`TxBuilder`]: super::tx_builder::TxBuilder
//!
//! ## Example
//!
//! ```no_run
//! # use std::str::FromStr;
//! # use bitcoin::*;
//! # use bitcoin::consensus::serialize;
//! # use bdk::wallet::coin_selection::*;
//! # use bdk::database::Database;
//! # use bdk::*;
//! #[derive(Debug)]
//! struct AlwaysSpendEverything;
//!
//! impl<D: Database> CoinSelectionAlgorithm<D> for AlwaysSpendEverything {
//!     fn coin_select(
//!         &self,
//!         database: &D,
//!         must_use_utxos: Vec<(UTXO, usize)>,
//!         may_use_utxos: Vec<(UTXO, usize)>,
//!         fee_rate: FeeRate,
//!         amount_needed: u64,
//!         fee_amount: f32,
//!     ) -> Result<CoinSelectionResult, bdk::Error> {
//!         let mut selected_amount = 0;
//!         let mut additional_weight = 0;
//!         let all_utxos_selected = must_use_utxos
//!             .into_iter().chain(may_use_utxos)
//!             .scan((&mut selected_amount, &mut additional_weight), |(selected_amount, additional_weight), (utxo, weight)| {
//!                 let txin = TxIn {
//!                     previous_output: utxo.outpoint,
//!                     ..Default::default()
//!                 };
//!
//!                 **selected_amount += utxo.txout.value;
//!                 **additional_weight += serialize(&txin).len() * 4 + weight;
//!
//!                 Some((
//!                     txin,
//!                     utxo.txout.script_pubkey,
//!                 ))
//!             })
//!             .collect::<Vec<_>>();
//!         let additional_fees = additional_weight as f32 * fee_rate.as_sat_vb() / 4.0;
//!
//!         if (fee_amount + additional_fees).ceil() as u64 + amount_needed > selected_amount {
//!             return Err(bdk::Error::InsufficientFunds);
//!         }
//!
//!         Ok(CoinSelectionResult {
//!             txin: all_utxos_selected,
//!             selected_amount,
//!             fee_amount: fee_amount + additional_fees,
//!         })
//!     }
//! }
//!
//! # let wallet: OfflineWallet<_> = Wallet::new_offline("", None, Network::Testnet, bdk::database::MemoryDatabase::default())?;
//! // create wallet, sync, ...
//!
//! let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
//! let (psbt, details) = wallet.create_tx(
//!     TxBuilder::with_recipients(vec![(to_address.script_pubkey(), 50_000)])
//!         .coin_selection(AlwaysSpendEverything),
//! )?;
//!
//! // inspect, sign, broadcast, ...
//!
//! # Ok::<(), bdk::Error>(())
//! ```

use bitcoin::consensus::encode::serialize;
use bitcoin::{Script, TxIn};

use crate::database::Database;
use crate::error::Error;
use crate::types::{FeeRate, UTXO};

/// Default coin selection algorithm used by [`TxBuilder`](super::tx_builder::TxBuilder) if not
/// overridden
pub type DefaultCoinSelectionAlgorithm = DumbCoinSelection;

/// Result of a successful coin selection
#[derive(Debug)]
pub struct CoinSelectionResult {
    /// List of inputs to use, with the respective previous script_pubkey
    pub txin: Vec<(TxIn, Script)>,
    /// Sum of the selected inputs' value
    pub selected_amount: u64,
    /// Total fee amount in satoshi
    pub fee_amount: f32,
}

/// Trait for generalized coin selection algorithms
///
/// This trait can be implemented to make the [`Wallet`](super::Wallet) use a customized coin
/// selection algorithm when it creates transactions.
///
/// For an example see [this module](crate::wallet::coin_selection)'s documentation.
pub trait CoinSelectionAlgorithm<D: Database>: std::fmt::Debug {
    /// Perform the coin selection
    ///
    /// - `database`: a reference to the wallet's database that can be used to lookup additional
    ///               details for a specific UTXO
    /// - `must_use_utxos`: the utxos that must be spent regardless of `amount_needed` with their
    ///                     weight cost
    /// - `may_be_spent`: the utxos that may be spent to satisfy `amount_needed` with their weight
    ///                   cost
    /// - `fee_rate`: fee rate to use
    /// - `amount_needed`: the amount in satoshi to select
    /// - `fee_amount`: the amount of fees in satoshi already accumulated from adding outputs and
    ///                 the transaction's header
    fn coin_select(
        &self,
        database: &D,
        must_use_utxos: Vec<(UTXO, usize)>,
        may_use_utxos: Vec<(UTXO, usize)>,
        fee_rate: FeeRate,
        amount_needed: u64,
        fee_amount: f32,
    ) -> Result<CoinSelectionResult, Error>;
}

/// Simple and dumb coin selection
///
/// This coin selection algorithm sorts the available UTXOs by value and then picks them starting
/// from the largest ones until the required amount is reached.
#[derive(Debug, Default)]
pub struct DumbCoinSelection;

impl<D: Database> CoinSelectionAlgorithm<D> for DumbCoinSelection {
    fn coin_select(
        &self,
        _database: &D,
        must_use_utxos: Vec<(UTXO, usize)>,
        mut may_use_utxos: Vec<(UTXO, usize)>,
        fee_rate: FeeRate,
        amount_needed: u64,
        mut fee_amount: f32,
    ) -> Result<CoinSelectionResult, Error> {
        let calc_fee_bytes = |wu| (wu as f32) * fee_rate.as_sat_vb() / 4.0;

        log::debug!(
            "amount_needed = `{}`, fee_amount = `{}`, fee_rate = `{:?}`",
            amount_needed,
            fee_amount,
            fee_rate
        );

        // We put the "must_use" UTXOs first and make sure the "may_use" are sorted, initially
        // smallest to largest, before being reversed with `.rev()`.
        let utxos = {
            may_use_utxos.sort_unstable_by_key(|(utxo, _)| utxo.txout.value);
            must_use_utxos
                .into_iter()
                .map(|utxo| (true, utxo))
                .chain(may_use_utxos.into_iter().rev().map(|utxo| (false, utxo)))
        };

        // Keep including inputs until we've got enough.
        // Store the total input value in selected_amount and the total fee being paid in fee_amount
        let mut selected_amount = 0;
        let txin = utxos
            .scan(
                (&mut selected_amount, &mut fee_amount),
                |(selected_amount, fee_amount), (must_use, (utxo, weight))| {
                    if must_use || **selected_amount < amount_needed + (fee_amount.ceil() as u64) {
                        let new_in = TxIn {
                            previous_output: utxo.outpoint,
                            script_sig: Script::default(),
                            sequence: 0, // Let the caller choose the right nSequence
                            witness: vec![],
                        };

                        **fee_amount += calc_fee_bytes(serialize(&new_in).len() * 4 + weight);
                        **selected_amount += utxo.txout.value;

                        log::debug!(
                            "Selected {}, updated fee_amount = `{}`",
                            new_in.previous_output,
                            fee_amount
                        );

                        Some((new_in, utxo.txout.script_pubkey))
                    } else {
                        None
                    }
                },
            )
            .collect::<Vec<_>>();

        if selected_amount < amount_needed + (fee_amount.ceil() as u64) {
            return Err(Error::InsufficientFunds);
        }

        Ok(CoinSelectionResult {
            txin,
            fee_amount,
            selected_amount,
        })
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::{OutPoint, Script, TxOut};

    use super::*;
    use crate::database::MemoryDatabase;
    use crate::types::*;

    const P2WPKH_WITNESS_SIZE: usize = 73 + 33 + 2;

    fn get_test_utxos() -> Vec<(UTXO, usize)> {
        vec![
            (
                UTXO {
                    outpoint: OutPoint::from_str(
                        "ebd9813ecebc57ff8f30797de7c205e3c7498ca950ea4341ee51a685ff2fa30a:0",
                    )
                    .unwrap(),
                    txout: TxOut {
                        value: 100_000,
                        script_pubkey: Script::new(),
                    },
                    is_internal: false,
                },
                P2WPKH_WITNESS_SIZE,
            ),
            (
                UTXO {
                    outpoint: OutPoint::from_str(
                        "65d92ddff6b6dc72c89624a6491997714b90f6004f928d875bc0fd53f264fa85:0",
                    )
                    .unwrap(),
                    txout: TxOut {
                        value: 200_000,
                        script_pubkey: Script::new(),
                    },
                    is_internal: true,
                },
                P2WPKH_WITNESS_SIZE,
            ),
        ]
    }

    #[test]
    fn test_dumb_coin_selection_success() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();

        let result = DumbCoinSelection::default()
            .coin_select(
                &database,
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                250_000,
                50.0,
            )
            .unwrap();

        assert_eq!(result.txin.len(), 2);
        assert_eq!(result.selected_amount, 300_000);
        assert_eq!(result.fee_amount, 186.0);
    }

    #[test]
    fn test_dumb_coin_selection_use_all() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();

        let result = DumbCoinSelection::default()
            .coin_select(
                &database,
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                50.0,
            )
            .unwrap();

        assert_eq!(result.txin.len(), 2);
        assert_eq!(result.selected_amount, 300_000);
        assert_eq!(result.fee_amount, 186.0);
    }

    #[test]
    fn test_dumb_coin_selection_use_only_necessary() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();

        let result = DumbCoinSelection::default()
            .coin_select(
                &database,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                50.0,
            )
            .unwrap();

        assert_eq!(result.txin.len(), 1);
        assert_eq!(result.selected_amount, 200_000);
        assert_eq!(result.fee_amount, 118.0);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_dumb_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();

        DumbCoinSelection::default()
            .coin_select(
                &database,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                500_000,
                50.0,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_dumb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();

        DumbCoinSelection::default()
            .coin_select(
                &database,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                250_000,
                50.0,
            )
            .unwrap();
    }
}
