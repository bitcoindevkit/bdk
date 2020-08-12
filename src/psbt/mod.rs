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
