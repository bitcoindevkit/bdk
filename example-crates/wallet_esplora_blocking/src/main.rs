const DB_MAGIC: &str = "bdk_wallet_esplora_example";
const SEND_AMOUNT: Amount = Amount::from_sat(1000);
const STOP_GAP: usize = 5;
const PARALLEL_REQUESTS: usize = 1;

use std::{collections::BTreeSet, io::Write, str::FromStr};

use bdk_esplora::{esplora_client, EsploraExt};
use bdk_file_store::Store;
use bdk_wallet::{
    bitcoin::{Address, Amount, Network},
    KeychainKind, SignOptions, Wallet,
};

fn main() -> Result<(), anyhow::Error> {
    let db_path = std::env::temp_dir().join("bdk-esplora-example");
    let db =
        Store::<bdk_wallet::wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";

    let mut wallet = Wallet::new_or_load(
        external_descriptor,
        Some(internal_descriptor),
        db,
        Network::Testnet,
    )?;

    let address = wallet.next_unused_address(KeychainKind::External)?;
    println!("Generated Address: {}", address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client =
        esplora_client::Builder::new("https://blockstream.info/testnet/api").build_blocking();

    let request = wallet.start_full_scan().inspect_spks_for_all_keychains({
        let mut once = BTreeSet::<KeychainKind>::new();
        move |keychain, spk_i, _| {
            match once.insert(keychain) {
                true => print!("\nScanning keychain [{:?}]", keychain),
                false => print!(" {:<3}", spk_i),
            };
            std::io::stdout().flush().expect("must flush")
        }
    });

    let mut update = client.full_scan(request, STOP_GAP, PARALLEL_REQUESTS)?;
    let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let _ = update.graph_update.update_last_seen_unconfirmed(now);

    wallet.apply_update(update)?;
    wallet.commit()?;
    println!();

    let balance = wallet.balance();
    println!("Wallet balance after syncing: {} sats", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} sats to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let faucet_address = Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?
        .require_network(Network::Testnet)?;

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(faucet_address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let mut psbt = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx()?;
    client.broadcast(&tx)?;
    println!("Tx broadcasted! Txid: {}", tx.txid());

    Ok(())
}
