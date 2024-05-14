use std::io::Write;

use bdk_electrum::{electrum_client, BdkElectrumClient};
use bdk_wallet::{
    bitcoin::{Amount, Network},
    chain::collections::HashSet,
    rusqlite::Connection,
    KeychainKind, SignOptions, Wallet,
};

const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 20;
const BATCH_SIZE: usize = 5;

const NETWORK: Network = Network::Signet;
const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
const INTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
const ELECTRUM_URL: &str = "ssl://mempool.space:60602";

fn main() -> Result<(), anyhow::Error> {
    let mut conn = Connection::open_in_memory().expect("must open connection");

    let wallet_opt = Wallet::load()
        .descriptors(EXTERNAL_DESC, INTERNAL_DESC)
        .network(NETWORK)
        .load_wallet(&mut conn)?;
    let mut wallet = match wallet_opt {
        Some(wallet) => wallet,
        None => Wallet::create(EXTERNAL_DESC, INTERNAL_DESC)
            .network(NETWORK)
            .create_wallet(&mut conn)?,
    };

    let address = wallet.next_unused_address(KeychainKind::External);
    wallet.persist(&mut conn)?;
    println!("Generated Address: {}", address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {}", balance.total());

    print!("Syncing...");
    let client = BdkElectrumClient::new(electrum_client::Client::new(ELECTRUM_URL)?);

    // Populate the electrum client's transaction cache so it doesn't redownload transaction we
    // already have.
    client.populate_tx_cache(wallet.tx_graph());

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

    let mut update = client.full_scan(request, STOP_GAP, BATCH_SIZE, false)?;

    let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let _ = update.graph_update.update_last_seen_unconfirmed(now);

    println!();

    wallet.apply_update(update)?;
    wallet.persist(&mut conn)?;

    let balance = wallet.balance();
    println!("Wallet balance after syncing: {}", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let mut psbt = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx()?;
    client.transaction_broadcast(&tx)?;
    println!("Tx broadcasted! Txid: {}", tx.compute_txid());

    Ok(())
}
