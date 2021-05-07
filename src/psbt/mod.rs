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

use bitcoin::util::psbt::PartiallySignedTransaction as Psbt;
use bitcoin::TxOut;

pub trait PsbtUtils {
    fn get_utxo_for(&self, input_index: usize) -> Option<TxOut>;
}

impl PsbtUtils for Psbt {
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

#[cfg(test)]
mod test {
    use crate::bitcoin::consensus::deserialize;
    use crate::bitcoin::TxIn;
    use crate::psbt::Psbt;
    use crate::wallet::test::{get_funded_wallet, get_test_wpkh};
    use crate::wallet::AddressIndex;
    use crate::SignOptions;

    // from bip 174
    const PSBT_STR: &str = "cHNidP8BAKACAAAAAqsJSaCMWvfEm4IS9Bfi8Vqz9cM9zxU4IagTn4d6W3vkAAAAAAD+////qwlJoIxa98SbghL0F+LxWrP1wz3PFTghqBOfh3pbe+QBAAAAAP7///8CYDvqCwAAAAAZdqkUdopAu9dAy+gdmI5x3ipNXHE5ax2IrI4kAAAAAAAAGXapFG9GILVT+glechue4O/p+gOcykWXiKwAAAAAAAEHakcwRAIgR1lmF5fAGwNrJZKJSGhiGDR9iYZLcZ4ff89X0eURZYcCIFMJ6r9Wqk2Ikf/REf3xM286KdqGbX+EhtdVRs7tr5MZASEDXNxh/HupccC1AaZGoqg7ECy0OIEhfKaC3Ibi1z+ogpIAAQEgAOH1BQAAAAAXqRQ1RebjO4MsRwUPJNPuuTycA5SLx4cBBBYAFIXRNTfy4mVAWjTbr6nj3aAfuCMIAAAA";

    #[test]
    #[should_panic(expected = "InputIndexOutOfRange")]
    fn test_psbt_malformed_psbt_input_legacy() {
        let psbt_bip: Psbt = deserialize(&base64::decode(PSBT_STR).unwrap()).unwrap();
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();
        psbt.inputs.push(psbt_bip.inputs[0].clone());
        let options = SignOptions {
            trust_witness_utxo: true,
            assume_height: None,
        };
        let _ = wallet.sign(&mut psbt, options).unwrap();
    }

    #[test]
    #[should_panic(expected = "InputIndexOutOfRange")]
    fn test_psbt_malformed_psbt_input_segwit() {
        let psbt_bip: Psbt = deserialize(&base64::decode(PSBT_STR).unwrap()).unwrap();
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();
        psbt.inputs.push(psbt_bip.inputs[1].clone());
        let options = SignOptions {
            trust_witness_utxo: true,
            assume_height: None,
        };
        let _ = wallet.sign(&mut psbt, options).unwrap();
    }

    #[test]
    #[should_panic(expected = "InputIndexOutOfRange")]
    fn test_psbt_malformed_tx_input() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();
        psbt.global.unsigned_tx.input.push(TxIn::default());
        let options = SignOptions {
            trust_witness_utxo: true,
            assume_height: None,
        };
        let _ = wallet.sign(&mut psbt, options).unwrap();
    }

    #[test]
    fn test_psbt_sign_with_finalized() {
        let psbt_bip: Psbt = deserialize(&base64::decode(PSBT_STR).unwrap()).unwrap();
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();

        // add a finalized input
        psbt.inputs.push(psbt_bip.inputs[0].clone());
        psbt.global
            .unsigned_tx
            .input
            .push(psbt_bip.global.unsigned_tx.input[0].clone());

        let _ = wallet.sign(&mut psbt, SignOptions::default()).unwrap();
    }
}
