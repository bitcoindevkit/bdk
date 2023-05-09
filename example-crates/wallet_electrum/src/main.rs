// use std::{io::Write, str::FromStr};

// use bdk::{
//     bitcoin::{Address, Network},
//     SignOptions, Wallet,
// };
// use bdk_electrum::{
//     electrum_client::{self, ElectrumApi},
//     ElectrumExt,
// };
// use bdk_file_store::KeychainStore;

// const SEND_AMOUNT: u64 = 5000;
// const STOP_GAP: usize = 50;
// const BATCH_SIZE: usize = 5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    todo!("update this example!");
    // println!("Hello, world!");

    // let db_path = std::env::temp_dir().join("bdk-electrum-example");
    // let db = KeychainStore::new_from_path(db_path)?;
    // let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/0/*)";
    // let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/1/*)";

    // let mut wallet = Wallet::new(
    //     external_descriptor,
    //     Some(internal_descriptor),
    //     db,
    //     Network::Testnet,
    // )?;

    // let address = wallet.get_address(bdk::wallet::AddressIndex::New);
    // println!("Generated Address: {}", address);

    // let balance = wallet.get_balance();
    // println!("Wallet balance before syncing: {} sats", balance.total());

    // print!("Syncing...");
    // // Scanning the chain...
    // let electrum_url = "ssl://electrum.blockstream.info:60002";
    // let client = electrum_client::Client::new(electrum_url)?;
    // let local_chain = wallet.checkpoints();
    // let spks = wallet
    //     .spks_of_all_keychains()
    //     .into_iter()
    //     .map(|(k, spks)| {
    //         let mut first = true;
    //         (
    //             k,
    //             spks.inspect(move |(spk_i, _)| {
    //                 if first {
    //                     first = false;
    //                     print!("\nScanning keychain [{:?}]:", k);
    //                 }
    //                 print!(" {}", spk_i);
    //                 let _ = std::io::stdout().flush();
    //             }),
    //         )
    //     })
    //     .collect();
    // let electrum_update = client
    //     .scan(
    //         local_chain,
    //         spks,
    //         core::iter::empty(),
    //         core::iter::empty(),
    //         STOP_GAP,
    //         BATCH_SIZE,
    //     )?
    //     .into_confirmation_time_update(&client)?;
    // println!();
    // let new_txs = client.batch_transaction_get(electrum_update.missing_full_txs(&wallet))?;
    // let update = electrum_update.into_keychain_scan(new_txs, &wallet)?;
    // wallet.apply_update(update)?;
    // wallet.commit()?;

    // let balance = wallet.get_balance();
    // println!("Wallet balance after syncing: {} sats", balance.total());

    // if balance.total() < SEND_AMOUNT {
    //     println!(
    //         "Please send at least {} sats to the receiving address",
    //         SEND_AMOUNT
    //     );
    //     std::process::exit(0);
    // }

    // let faucet_address = Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?;

    // let mut tx_builder = wallet.build_tx();
    // tx_builder
    //     .add_recipient(faucet_address.script_pubkey(), SEND_AMOUNT)
    //     .enable_rbf();

    // let (mut psbt, _) = tx_builder.finish()?;
    // let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    // assert!(finalized);

    // let tx = psbt.extract_tx();
    // client.transaction_broadcast(&tx)?;
    // println!("Tx broadcasted! Txid: {}", tx.txid());

    // Ok(())
}
