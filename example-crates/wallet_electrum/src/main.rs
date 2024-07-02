use std::io::Write;
use std::str::FromStr;

use bdk_electrum::{electrum_client, BdkElectrumClient};
use bdk_wallet::{
    bitcoin::{Address, Amount, Network},
    chain::collections::HashSet,
    KeychainKind, SignOptions, Wallet,
};

use bdk_sqlite::{rusqlite::Connection, Store};

const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 20;
const BATCH_SIZE: usize = 5;

fn main() -> Result<(), anyhow::Error> {
    let conn = Connection::open_in_memory().expect("must open connection");
    let mut db = Store::new(conn).expect("must create db");
    let changeset = db.read()?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";

    let mut wallet = Wallet::new_or_load(
        external_descriptor,
        internal_descriptor,
        changeset,
        Network::Signet,
    )?;

    let address = wallet.next_unused_address(KeychainKind::External);
    println!("Generated Address: {}", address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {}", balance.total());

    print!("Syncing...");
    let client = BdkElectrumClient::new(electrum_client::Client::new("ssl://mempool.space:60602")?);

    let request = wallet
        .start_full_scan()
        .inspect_spks_for_all_keychains({
            let mut once = HashSet::<KeychainKind>::new();
            move |kind, spk_i, _| {
                if once.insert(kind) {
                    print!("\nScanning keychain [{:?}] {:<3}", kind, spk_i)
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

    let balance = wallet.balance();
    println!("Wallet balance after syncing: {}", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let faucet_address = Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?
        .require_network(Network::Signet)?;

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
