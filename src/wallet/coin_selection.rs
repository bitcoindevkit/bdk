use bitcoin::consensus::encode::serialize;
use bitcoin::{Script, TxIn};

use crate::error::Error;
use crate::types::UTXO;

pub type DefaultCoinSelectionAlgorithm = DumbCoinSelection;

#[derive(Debug)]
pub struct CoinSelectionResult {
    pub txin: Vec<(TxIn, Script)>,
    pub total_amount: u64,
    pub fee_amount: f32,
}

pub trait CoinSelectionAlgorithm: std::fmt::Debug {
    fn coin_select(
        &self,
        utxos: Vec<UTXO>,
        use_all_utxos: bool,
        fee_rate: f32,
        outgoing_amount: u64,
        input_witness_weight: usize,
        fee_amount: f32,
    ) -> Result<CoinSelectionResult, Error>;
}

#[derive(Debug, Default)]
pub struct DumbCoinSelection;

impl CoinSelectionAlgorithm for DumbCoinSelection {
    fn coin_select(
        &self,
        mut utxos: Vec<UTXO>,
        use_all_utxos: bool,
        fee_rate: f32,
        outgoing_amount: u64,
        input_witness_weight: usize,
        mut fee_amount: f32,
    ) -> Result<CoinSelectionResult, Error> {
        let mut txin = Vec::new();
        let calc_fee_bytes = |wu| (wu as f32) * fee_rate / 4.0;

        log::debug!(
            "outgoing_amount = `{}`, fee_amount = `{}`, fee_rate = `{}`",
            outgoing_amount,
            fee_amount,
            fee_rate
        );

        // sort so that we pick them starting from the larger.
        utxos.sort_by(|a, b| a.txout.value.partial_cmp(&b.txout.value).unwrap());

        let mut total_amount: u64 = 0;
        while use_all_utxos || total_amount < outgoing_amount + (fee_amount.ceil() as u64) {
            let utxo = match utxos.pop() {
                Some(utxo) => utxo,
                None if total_amount < outgoing_amount + (fee_amount.ceil() as u64) => {
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
            total_amount += utxo.txout.value;
        }

        Ok(CoinSelectionResult {
            txin,
            fee_amount,
            total_amount,
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
            .coin_select(utxos, false, 1.0, 250_000, P2WPKH_WITNESS_SIZE, 50.0)
            .unwrap();

        assert_eq!(result.txin.len(), 2);
        assert_eq!(result.total_amount, 300_000);
        assert_eq!(result.fee_amount, 186.0);
    }

    #[test]
    fn test_dumb_coin_selection_use_all() {
        let utxos = get_test_utxos();

        let result = DumbCoinSelection
            .coin_select(utxos, true, 1.0, 20_000, P2WPKH_WITNESS_SIZE, 50.0)
            .unwrap();

        assert_eq!(result.txin.len(), 2);
        assert_eq!(result.total_amount, 300_000);
        assert_eq!(result.fee_amount, 186.0);
    }

    #[test]
    fn test_dumb_coin_selection_use_only_necessary() {
        let utxos = get_test_utxos();

        let result = DumbCoinSelection
            .coin_select(utxos, false, 1.0, 20_000, P2WPKH_WITNESS_SIZE, 50.0)
            .unwrap();

        assert_eq!(result.txin.len(), 1);
        assert_eq!(result.total_amount, 200_000);
        assert_eq!(result.fee_amount, 118.0);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_dumb_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();

        DumbCoinSelection
            .coin_select(utxos, false, 1.0, 500_000, P2WPKH_WITNESS_SIZE, 50.0)
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_dumb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();

        DumbCoinSelection
            .coin_select(utxos, false, 1000.0, 250_000, P2WPKH_WITNESS_SIZE, 50.0)
            .unwrap();
    }
}
