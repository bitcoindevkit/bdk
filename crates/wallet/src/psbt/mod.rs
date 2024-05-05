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

use alloc::vec::Vec;
use bitcoin::Amount;
use bitcoin::FeeRate;
use bitcoin::Psbt;
use bitcoin::TxOut;

// TODO upstream the functions here to `rust-bitcoin`?

/// Trait to add functions to extract utxos and calculate fees.
pub trait PsbtUtils {
    /// Get the `TxOut` for the specified input index, if it doesn't exist in the PSBT `None` is returned.
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut>;

    /// The total transaction fee amount, sum of input amounts minus sum of output amounts, in sats.
    /// If the PSBT is missing a TxOut for an input returns None.
    fn fee_amount(&self) -> Option<Amount>;

    /// The transaction's fee rate. This value will only be accurate if calculated AFTER the
    /// `Psbt` is finalized and all witness/signature data is added to the
    /// transaction.
    /// If the PSBT is missing a TxOut for an input returns None.
    fn fee_rate(&self) -> Option<FeeRate>;
}

impl PsbtUtils for Psbt {
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut> {
        let tx = &self.unsigned_tx;
        let input = self.inputs.get(input_index)?;

        match (&input.witness_utxo, &input.non_witness_utxo) {
            (Some(_), _) => input.witness_utxo.clone(),
            (_, Some(_)) => input.non_witness_utxo.as_ref().map(|in_tx| {
                in_tx.output[tx.input[input_index].previous_output.vout as usize].clone()
            }),
            _ => None,
        }
    }

    fn fee_amount(&self) -> Option<Amount> {
        let tx = &self.unsigned_tx;
        let utxos: Option<Vec<TxOut>> = (0..tx.input.len()).map(|i| self.get_utxo_for(i)).collect();

        utxos.map(|inputs| {
            let input_amount: Amount = inputs.iter().map(|i| i.value).sum();
            let output_amount: Amount = self.unsigned_tx.output.iter().map(|o| o.value).sum();
            input_amount
                .checked_sub(output_amount)
                .expect("input amount must be greater than output amount")
        })
    }

    fn fee_rate(&self) -> Option<FeeRate> {
        let fee_amount = self.fee_amount();
        let weight = self.clone().extract_tx().ok()?.weight();
        fee_amount.map(|fee| fee / weight)
    }
}
