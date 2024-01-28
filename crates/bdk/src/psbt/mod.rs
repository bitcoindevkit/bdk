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

//! Additional functions on the `rust-bitcoin` `Psbt` structure.

use crate::FeeRate;
use alloc::vec::Vec;
use bitcoin::psbt::Psbt;
use bitcoin::TxOut;

// TODO upstream the functions here to `rust-bitcoin`?

/// Trait to add functions to extract utxos and calculate fees.
pub trait PsbtUtils {
    /// Get the `TxOut` for the specified input index, if it doesn't exist in the PSBT `None` is returned.
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut>;

    /// The total transaction fee amount, sum of input amounts minus sum of output amounts, in sats.
    /// If the PSBT is missing a TxOut for an input returns None.
    fn fee_amount(&self) -> Option<u64>;

    /// The transaction's fee rate. This value will only be accurate if calculated AFTER the
    /// `Psbt` is finalized and all witness/signature data is added to the
    /// transaction.
    /// If the PSBT is missing a TxOut for an input returns None.
    fn fee_rate(&self) -> Option<FeeRate>;
}

impl PsbtUtils for Psbt {
    #[allow(clippy::all)] // We want to allow `manual_map` but it is too new.
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut> {
        let tx = &self.unsigned_tx;

        if input_index >= tx.input.len() {
            return None;
        }

        if let Some(input) = self.inputs.get(input_index) {
            if let Some(wit_utxo) = &input.witness_utxo {
                Some(wit_utxo.clone())
            } else if let Some(in_tx) = &input.non_witness_utxo {
                Some(in_tx.output[tx.input[input_index].previous_output.vout as usize].clone())
            } else {
                None
            }
        } else {
            None
        }
    }

    fn fee_amount(&self) -> Option<u64> {
        let tx = &self.unsigned_tx;
        let utxos: Option<Vec<TxOut>> = (0..tx.input.len()).map(|i| self.get_utxo_for(i)).collect();

        utxos.map(|inputs| {
            let input_amount: u64 = inputs.iter().map(|i| i.value.to_btc() as u64).sum();
            let output_amount: u64 = self
                .unsigned_tx
                .output
                .iter()
                .map(|o| o.value.to_btc() as u64)
                .sum();
            input_amount
                .checked_sub(output_amount)
                .expect("input amount must be greater than output amount")
        })
    }

    fn fee_rate(&self) -> Option<FeeRate> {
        let fee_amount = self.fee_amount();
        fee_amount.map(|fee| {
            let weight = self.clone().extract_tx_unchecked_fee_rate().weight();
            FeeRate::from_wu(fee, weight)
        })
    }
}
