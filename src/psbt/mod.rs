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

use bitcoin::{util::psbt::PartiallySignedTransaction as PSBT, TxOut};

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
