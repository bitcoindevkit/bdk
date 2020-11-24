use crate::UTXO;
use bitcoin::OutPoint;
use bitcoin::TxOut;

/// A foregin utxo is a utxo that is not owned by the wallet.
///
/// For use in [`TxBuilder::add_foreign_utxo`]. You should take this information from a possibly
/// malicious party without verifying it against the actual blockchain. This is especially true for
/// the `txout.value` but may also be wise for the `satisfaction_weight` if security depends on
/// guaranteeing a minimum feerate for the transaction.
///
/// Foreign UTXOs are currently experimental and only work if the `txout.script_pubkey` is a witness
/// program.
#[derive(Clone, Debug, PartialEq)]
pub struct ForeignUtxo {
    /// The `TxOut` for the utxo, required since we need to know the value and scriptPubKey.
    pub txout: TxOut,
    /// The location of the output.
    pub outpoint: OutPoint,
    /// The weight that the other party will add to the transaction when signing it.
    pub satisfaction_weight: usize,
}

impl ForeignUtxo {
    pub(crate) fn into_utxo_and_weight(self) -> (UTXO, usize) {
        (
            UTXO {
                outpoint: self.outpoint,
                txout: self.txout,
                is_internal: false,
            },
            self.satisfaction_weight,
        )
    }
}
