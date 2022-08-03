pub(crate) mod tx {

    use std::str::FromStr;

    use bdk::{database::BatchDatabase, SignOptions, Wallet};
    use bitcoin::{Address, Transaction};

    pub fn build_signed_tx<D: BatchDatabase>(
        wallet: &Wallet<D>,
        recipient_address: &str,
        amount: u64,
    ) -> Transaction {
        // Create a transaction builder
        let mut tx_builder = wallet.build_tx();

        let to_address = Address::from_str(recipient_address).unwrap();

        // Set recipient of the transaction
        tx_builder.set_recipients(vec![(to_address.script_pubkey(), amount)]);

        // Finalise the transaction and extract PSBT
        let (mut psbt, _) = tx_builder.finish().unwrap();

        // Sign the above psbt with signing option
        wallet.sign(&mut psbt, SignOptions::default()).unwrap();

        // Extract the final transaction
        psbt.extract_tx()
    }
}
