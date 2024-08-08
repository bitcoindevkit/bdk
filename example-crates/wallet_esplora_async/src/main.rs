use std::{collections::BTreeSet, io::Write};

use anyhow::Ok;
use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_wallet::{
    bitcoin::{Amount, Network},
    rusqlite::Connection,
    KeychainKind, SignOptions, Wallet,
};

const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 5;
const PARALLEL_REQUESTS: usize = 5;

const DB_PATH: &str = "bdk-example-esplora-async.sqlite";
const NETWORK: Network = Network::Signet;
const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
const INTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
const ESPLORA_URL: &str = "http://signet.bitcoindevkit.net";

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let mut conn = Connection::open(DB_PATH)?;

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

    let address = wallet.mutate(&mut conn, |w| w.next_unused_address(KeychainKind::External))?;
    println!("Next unused address: ({}) {}", address.index, address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client = esplora_client::Builder::new(ESPLORA_URL).build_async()?;

    let request = wallet.start_full_scan().inspect_spks_for_all_keychains({
        let mut once = BTreeSet::<KeychainKind>::new();
        move |keychain, spk_i, _| {
            if once.insert(keychain) {
                print!("\nScanning keychain [{:?}] ", keychain);
            }
            print!(" {:<3}", spk_i);
            std::io::stdout().flush().expect("must flush")
        }
    });

    let mut update = client
        .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
        .await?;
    let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let _ = update.graph_update.update_last_seen_unconfirmed(now);

    wallet.mutate(&mut conn, |w| w.apply_update(update.clone()))??;
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

    let mut psbt = wallet.mutate(&mut conn, |w| -> anyhow::Result<_> {
        let mut tx_builder = w.build_tx();
        tx_builder
            .add_recipient(address.script_pubkey(), SEND_AMOUNT)
            .enable_rbf();
        Ok(tx_builder.finish()?)
    })??;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx()?;
    client.broadcast(&tx).await?;
    println!("Tx broadcasted! Txid: {}", tx.compute_txid());

    Ok(())
}
