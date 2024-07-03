const DB_MAGIC: &str = "bdk_wallet_electrum_example";
const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 50;
const BATCH_SIZE: usize = 5;

use anyhow::anyhow;
use std::io::Write;
use std::str::FromStr;

use bdk_electrum::electrum_client;
use bdk_electrum::BdkElectrumClient;
use bdk_file_store::Store;
use bdk_wallet::bitcoin::{Address, Amount};
use bdk_wallet::chain::collections::HashSet;
use bdk_wallet::{bitcoin::Network, Wallet};
use bdk_wallet::{KeychainKind, SignOptions};

fn main() -> Result<(), anyhow::Error> {
    let db_path = std::env::temp_dir().join("bdk-electrum-example");
    let mut db =
        Store::<bdk_wallet::wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
    let changeset = db
        .aggregate_changesets()
        .map_err(|e| anyhow!("load changes error: {}", e))?;
    let mut wallet;

    if let Some(changeset) = changeset {
        wallet = Wallet::load(
            external_descriptor,
            internal_descriptor,
            changeset,
            Network::Testnet,
        )?;
    } else {
        wallet = Wallet::new(external_descriptor, internal_descriptor, Network::Testnet)?;
    }

    let address = wallet.next_unused_address(KeychainKind::External);
    if let Some(changeset) = wallet.take_staged() {
        db.append_changeset(&changeset)?;
    }
    println!("Generated Address: {}", address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client = BdkElectrumClient::new(electrum_client::Client::new(
        "ssl://electrum.blockstream.info:60002",
    )?);

    // Populate the electrum client's transaction cache so it doesn't redownload transaction we
    // already have.
    client.populate_tx_cache(&wallet);

    let request = wallet
        .start_full_scan()
        .inspect_spks_for_all_keychains({
            let mut once = HashSet::<KeychainKind>::new();
            move |k, spk_i, _| {
                if once.insert(k) {
                    print!("\nScanning keychain [{:?}]", k)
                } else {
                    print!(" {:<3}", spk_i)
                }
            }
        })
        .inspect_spks_for_all_keychains(|_, _, _| std::io::stdout().flush().expect("must flush"));

    let mut update = client
        .full_scan(request, STOP_GAP, BATCH_SIZE, false)?
        .with_confirmation_time_height_anchor(&client)?;

    let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let _ = update.graph_update.update_last_seen_unconfirmed(now);

    println!();

    wallet.apply_update(update)?;
    if let Some(changeset) = wallet.take_staged() {
        db.append_changeset(&changeset)?;
    }

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
