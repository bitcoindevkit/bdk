//! Common test helpers and macros

#![allow(unused)]

/// The satisfaction size of a P2WPKH is 112 WU =
/// 1 (elements in witness) + 1 (OP_PUSH) + 33 (pk) + 1 (OP_PUSH) + 72 (signature + sighash) + 1*4 (script len)
/// On the witness itself, we have to push once for the pk (33WU) and once for signature + sighash (72WU), for
/// a total of 105 WU.
/// Here, we push just once for simplicity, so we have to add an extra byte for the missing
/// OP_PUSH.
pub const P2WPKH_FAKE_WITNESS_SIZE: usize = 106;

#[macro_export]
macro_rules! check_fee {
    ($wallet:expr, $psbt: expr) => {{
        let tx = $psbt.clone().extract_tx().expect("failed to extract tx");
        let tx_fee = $wallet.calculate_fee(&tx).ok();
        assert_eq!(tx_fee, $psbt.fee_amount());
        tx_fee
    }};
}

#[macro_export]
macro_rules! assert_fee_rate {
    ($psbt:expr, $fees:expr, $fee_rate:expr $( ,@dust_change $( $dust_change:expr )* )* $( ,@add_signature $( $add_signature:expr )* )* ) => ({
        let psbt = $psbt.clone();
        #[allow(unused_mut)]
        let mut tx = $psbt.clone().extract_tx().expect("failed to extract tx");
        $(
            $( $add_signature )*
                for txin in &mut tx.input {
                    txin.witness.push([0x00; common::P2WPKH_FAKE_WITNESS_SIZE]);
                }
        )*

            #[allow(unused_mut)]
        #[allow(unused_assignments)]
        let mut dust_change = false;
        $(
            $( $dust_change )*
                dust_change = true;
        )*

            let fee_amount = psbt
            .inputs
            .iter()
            .fold(Amount::ZERO, |acc, i| acc + i.witness_utxo.as_ref().unwrap().value)
            - psbt
            .unsigned_tx
            .output
            .iter()
            .fold(Amount::ZERO, |acc, o| acc + o.value);

        assert_eq!(fee_amount, $fees);

        let tx_fee_rate = (fee_amount / tx.weight())
            .to_sat_per_kwu();
        let fee_rate = $fee_rate.to_sat_per_kwu();
        let half_default = FeeRate::BROADCAST_MIN.checked_div(2)
            .unwrap()
            .to_sat_per_kwu();

        if !dust_change {
            assert!(tx_fee_rate >= fee_rate && tx_fee_rate - fee_rate < half_default, "Expected fee rate of {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
        } else {
            assert!(tx_fee_rate >= fee_rate, "Expected fee rate of at least {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
        }
    });
}
