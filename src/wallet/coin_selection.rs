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
//! # use magical_bitcoin_wallet::wallet::coin_selection::*;
//! # use magical_bitcoin_wallet::*;
//! #[derive(Debug)]
//! struct AlwaysSpendEverything;
//!
//! impl CoinSelectionAlgorithm for AlwaysSpendEverything {
//!     fn coin_select(
//!         &self,
//!         utxos: Vec<UTXO>,
//!         _use_all_utxos: bool,
//!         fee_rate: FeeRate,
//!         amount_needed: u64,
//!         input_witness_weight: usize,
//!         fee_amount: f32,
//!     ) -> Result<CoinSelectionResult, magical_bitcoin_wallet::Error> {
//!         let selected_amount = utxos.iter().fold(0, |acc, utxo| acc + utxo.txout.value);
//!         let all_utxos_selected = utxos
//!             .into_iter()
//!             .map(|utxo| {
//!                 (
//!                     TxIn {
//!                         previous_output: utxo.outpoint,
//!                         ..Default::default()
//!                     },
//!                     utxo.txout.script_pubkey,
//!                 )
//!             })
//!             .collect::<Vec<_>>();
//!         let additional_weight = all_utxos_selected.iter().fold(0, |acc, (txin, _)| {
//!             acc + serialize(txin).len() * 4 + input_witness_weight
//!         });
//!         let additional_fees = additional_weight as f32 * fee_rate.as_sat_vb() / 4.0;
//!
//!         if (fee_amount + additional_fees).ceil() as u64 + amount_needed > selected_amount {
//!             return Err(magical_bitcoin_wallet::Error::InsufficientFunds);
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
//! # let wallet: OfflineWallet<_> = Wallet::new_offline("", None, Network::Testnet, magical_bitcoin_wallet::database::MemoryDatabase::default())?;
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
//! # Ok::<(), magical_bitcoin_wallet::Error>(())
//! ```

use bitcoin::consensus::encode::serialize;
use bitcoin::{Script, TxIn};

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
pub trait CoinSelectionAlgorithm: std::fmt::Debug {
    /// Perform the coin selection
    ///
    /// - `utxos`: the list of spendable UTXOs
    /// - `use_all_utxos`: if true all utxos should be spent unconditionally
    /// - `fee_rate`: fee rate to use
    /// - `amount_needed`: the amount in satoshi to select
    /// - `input_witness_weight`: the weight of an input's witness to keep into account for the fees
    /// - `fee_amount`: the amount of fees in satoshi already accumulated from adding outputs
    fn coin_select(
        &self,
        utxos: Vec<UTXO>,
        use_all_utxos: bool,
        fee_rate: FeeRate,
        amount_needed: u64,
        input_witness_weight: usize,
        fee_amount: f32,
    ) -> Result<CoinSelectionResult, Error>;
}

/// Simple and dumb coin selection
///
/// This coin selection algorithm sorts the available UTXOs by value and then picks them starting
/// from the largest ones until the required amount is reached.
#[derive(Debug, Default)]
pub struct DumbCoinSelection;

impl CoinSelectionAlgorithm for DumbCoinSelection {
    fn coin_select(
        &self,
        mut utxos: Vec<UTXO>,
        use_all_utxos: bool,
        fee_rate: FeeRate,
        outgoing_amount: u64,
        input_witness_weight: usize,
        mut fee_amount: f32,
    ) -> Result<CoinSelectionResult, Error> {
        let mut txin = Vec::new();
        let calc_fee_bytes = |wu| (wu as f32) * fee_rate.as_sat_vb() / 4.0;

        log::debug!(
            "outgoing_amount = `{}`, fee_amount = `{}`, fee_rate = `{:?}`",
            outgoing_amount,
            fee_amount,
            fee_rate
        );

        // sort so that we pick them starting from the larger.
        utxos.sort_by(|a, b| a.txout.value.partial_cmp(&b.txout.value).unwrap());

        let mut selected_amount: u64 = 0;
        while use_all_utxos || selected_amount < outgoing_amount + (fee_amount.ceil() as u64) {
            let utxo = match utxos.pop() {
                Some(utxo) => utxo,
                None if selected_amount < outgoing_amount + (fee_amount.ceil() as u64) => {
                    return Err(Error::InsufficientFunds)
                }
                None if use_all_utxos => break,
                None => return Err(Error::InsufficientFunds),
            };

            let new_in = TxIn {
                previous_output: utxo.outpoint,
                script_sig: Script::default(),
                sequence: 0, // Let the caller choose the right nSequence
                witness: vec![],
            };
            fee_amount += calc_fee_bytes(serialize(&new_in).len() * 4 + input_witness_weight);
            log::debug!(
                "Selected {}, updated fee_amount = `{}`",
                new_in.previous_output,
                fee_amount
            );

            txin.push((new_in, utxo.txout.script_pubkey));
            selected_amount += utxo.txout.value;
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
    use crate::types::*;

    const P2WPKH_WITNESS_SIZE: usize = 73 + 33 + 2;

    fn get_test_utxos() -> Vec<UTXO> {
        vec![
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
        ]
    }

    #[test]
    fn test_dumb_coin_selection_success() {
        let utxos = get_test_utxos();

        let result = DumbCoinSelection
            .coin_select(
                utxos,
                false,
                FeeRate::from_sat_per_vb(1.0),
                250_000,
                P2WPKH_WITNESS_SIZE,
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

        let result = DumbCoinSelection
            .coin_select(
                utxos,
                true,
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                P2WPKH_WITNESS_SIZE,
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

        let result = DumbCoinSelection
            .coin_select(
                utxos,
                false,
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                P2WPKH_WITNESS_SIZE,
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

        DumbCoinSelection
            .coin_select(
                utxos,
                false,
                FeeRate::from_sat_per_vb(1.0),
                500_000,
                P2WPKH_WITNESS_SIZE,
                50.0,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_dumb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();

        DumbCoinSelection
            .coin_select(
                utxos,
                false,
                FeeRate::from_sat_per_vb(1000.0),
                250_000,
                P2WPKH_WITNESS_SIZE,
                50.0,
            )
            .unwrap();
    }
}
