use bdk_wallet::file_store::Store;
use bdk_wallet::Wallet;
use std::io::Write;
use std::str::FromStr;

use bdk_electrum::electrum_client;
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::bitcoin::{Address, Amount};
use bdk_wallet::chain::collections::HashSet;
use bdk_wallet::{KeychainKind, SignOptions};

const DB_MAGIC: &str = "bdk_wallet_electrum_example";
const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 50;
const BATCH_SIZE: usize = 5;

const NETWORK: Network = Network::Testnet;
const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
const INTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
const ELECTRUM_URL: &str = "ssl://electrum.blockstream.info:60002";

fn main() -> Result<(), anyhow::Error> {
    let db_path = "bdk-electrum-example.db";

    let mut db = Store::<bdk_wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), db_path)?;

    let wallet_opt = Wallet::load()
        .network(NETWORK)
        .descriptor(KeychainKind::External, Some(EXTERNAL_DESC))
        .descriptor(KeychainKind::Internal, Some(INTERNAL_DESC))
        .extract_keys()
        .load_wallet(&mut db)?;
    let mut wallet = match wallet_opt {
        Some(wallet) => wallet,
        None => Wallet::create(EXTERNAL_DESC, INTERNAL_DESC)
            .network(NETWORK)
            .create_wallet(&mut db)?,
    };

    let address = wallet.next_unused_address(KeychainKind::External);
    wallet.persist(&mut db)?;
    println!("Generated Address: {}", address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client = BdkElectrumClient::new(electrum_client::Client::new(ELECTRUM_URL)?);

    // Populate the electrum client's transaction cache so it doesn't redownload transaction we
    // already have.
    client.populate_tx_cache(wallet.tx_graph());

    let request = wallet.start_full_scan().inspect({
        let mut stdout = std::io::stdout();
        let mut once = HashSet::<KeychainKind>::new();
        move |k, spk_i, _| {
            if once.insert(k) {
                print!("\nScanning keychain [{:?}]", k)
            } else {
                print!(" {:<3}", spk_i)
            }
            stdout.flush().expect("must flush");
        }
    });

    let mut update = client.full_scan(request, STOP_GAP, BATCH_SIZE, false)?;

    let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let _ = update.graph_update.update_last_seen_unconfirmed(now);

    println!();

    wallet.apply_update(update)?;
    wallet.persist(&mut db)?;

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
    client.transaction_broadcast(&tx)?;
    println!("Tx broadcasted! Txid: {}", tx.compute_txid());

    Ok(())
}
