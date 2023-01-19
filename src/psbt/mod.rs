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

//! Additional functions on the `rust-bitcoin` `PartiallySignedTransaction` structure.

use crate::FeeRate;
use bitcoin::util::psbt::PartiallySignedTransaction as Psbt;
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
    /// `PartiallySignedTransaction` is finalized and all witness/signature data is added to the
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
            let input_amount: u64 = inputs.iter().map(|i| i.value).sum();
            let output_amount: u64 = self.unsigned_tx.output.iter().map(|o| o.value).sum();
            input_amount
                .checked_sub(output_amount)
                .expect("input amount must be greater than output amount")
        })
    }

    fn fee_rate(&self) -> Option<FeeRate> {
        let fee_amount = self.fee_amount();
        fee_amount.map(|fee| {
            let weight = self.clone().extract_tx().weight();
            FeeRate::from_wu(fee, weight)
        })
    }
}

#[cfg(test)]
mod test {
    use crate::bitcoin::TxIn;
    use crate::psbt::Psbt;
    use crate::wallet::AddressIndex;
    use crate::wallet::AddressIndex::New;
    use crate::wallet::{get_funded_wallet, test::get_test_wpkh};
    use crate::{psbt, FeeRate, SignOptions};
    use std::str::FromStr;

    // from bip 174
    const PSBT_STR: &str = "cHNidP8BAKACAAAAAqsJSaCMWvfEm4IS9Bfi8Vqz9cM9zxU4IagTn4d6W3vkAAAAAAD+////qwlJoIxa98SbghL0F+LxWrP1wz3PFTghqBOfh3pbe+QBAAAAAP7///8CYDvqCwAAAAAZdqkUdopAu9dAy+gdmI5x3ipNXHE5ax2IrI4kAAAAAAAAGXapFG9GILVT+glechue4O/p+gOcykWXiKwAAAAAAAEHakcwRAIgR1lmF5fAGwNrJZKJSGhiGDR9iYZLcZ4ff89X0eURZYcCIFMJ6r9Wqk2Ikf/REf3xM286KdqGbX+EhtdVRs7tr5MZASEDXNxh/HupccC1AaZGoqg7ECy0OIEhfKaC3Ibi1z+ogpIAAQEgAOH1BQAAAAAXqRQ1RebjO4MsRwUPJNPuuTycA5SLx4cBBBYAFIXRNTfy4mVAWjTbr6nj3aAfuCMIAAAA";

    #[test]
    #[should_panic(expected = "InputIndexOutOfRange")]
    fn test_psbt_malformed_psbt_input_legacy() {
        let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();
        psbt.inputs.push(psbt_bip.inputs[0].clone());
        let options = SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };
        let _ = wallet.sign(&mut psbt, options).unwrap();
    }

    #[test]
    #[should_panic(expected = "InputIndexOutOfRange")]
    fn test_psbt_malformed_psbt_input_segwit() {
        let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();
        psbt.inputs.push(psbt_bip.inputs[1].clone());
        let options = SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
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
        psbt.unsigned_tx.input.push(TxIn::default());
        let options = SignOptions {
            trust_witness_utxo: true,
            ..Default::default()
        };
        let _ = wallet.sign(&mut psbt, options).unwrap();
    }

    #[test]
    fn test_psbt_sign_with_finalized() {
        let psbt_bip = Psbt::from_str(PSBT_STR).unwrap();
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let send_to = wallet.get_address(AddressIndex::New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(send_to.script_pubkey(), 10_000);
        let (mut psbt, _) = builder.finish().unwrap();

        // add a finalized input
        psbt.inputs.push(psbt_bip.inputs[0].clone());
        psbt.unsigned_tx
            .input
            .push(psbt_bip.unsigned_tx.input[0].clone());

        let _ = wallet.sign(&mut psbt, SignOptions::default()).unwrap();
    }

    #[test]
    fn test_psbt_fee_rate_with_witness_utxo() {
        use psbt::PsbtUtils;

        let expected_fee_rate = 1.2345;

        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
        let (mut psbt, _) = builder.finish().unwrap();
        let fee_amount = psbt.fee_amount();
        assert!(fee_amount.is_some());

        let unfinalized_fee_rate = psbt.fee_rate().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let finalized_fee_rate = psbt.fee_rate().unwrap();
        assert!(finalized_fee_rate.as_sat_per_vb() >= expected_fee_rate);
        assert!(finalized_fee_rate.as_sat_per_vb() < unfinalized_fee_rate.as_sat_per_vb());
    }

    #[test]
    fn test_psbt_fee_rate_with_nonwitness_utxo() {
        use psbt::PsbtUtils;

        let expected_fee_rate = 1.2345;

        let (wallet, _, _) = get_funded_wallet("pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
        let (mut psbt, _) = builder.finish().unwrap();
        let fee_amount = psbt.fee_amount();
        assert!(fee_amount.is_some());
        let unfinalized_fee_rate = psbt.fee_rate().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let finalized_fee_rate = psbt.fee_rate().unwrap();
        assert!(finalized_fee_rate.as_sat_per_vb() >= expected_fee_rate);
        assert!(finalized_fee_rate.as_sat_per_vb() < unfinalized_fee_rate.as_sat_per_vb());
    }

    #[test]
    fn test_psbt_fee_rate_with_missing_txout() {
        use psbt::PsbtUtils;

        let expected_fee_rate = 1.2345;

        let (wpkh_wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wpkh_wallet.get_address(New).unwrap();
        let mut builder = wpkh_wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
        let (mut wpkh_psbt, _) = builder.finish().unwrap();

        wpkh_psbt.inputs[0].witness_utxo = None;
        wpkh_psbt.inputs[0].non_witness_utxo = None;
        assert!(wpkh_psbt.fee_amount().is_none());
        assert!(wpkh_psbt.fee_rate().is_none());

        let (pkh_wallet, _, _) = get_funded_wallet("pkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = pkh_wallet.get_address(New).unwrap();
        let mut builder = pkh_wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        builder.fee_rate(FeeRate::from_sat_per_vb(expected_fee_rate));
        let (mut pkh_psbt, _) = builder.finish().unwrap();

        pkh_psbt.inputs[0].non_witness_utxo = None;
        assert!(pkh_psbt.fee_amount().is_none());
        assert!(pkh_psbt.fee_rate().is_none());
    }
}
