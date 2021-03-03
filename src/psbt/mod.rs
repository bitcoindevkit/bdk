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

use bitcoin::util::psbt::PartiallySignedTransaction as PSBT;
use bitcoin::TxOut;

pub trait PSBTUtils {
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut>;
}

impl PSBTUtils for PSBT {
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut> {
        let tx = &self.global.unsigned_tx;

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
}
