use std::{io::Write, str::FromStr};

use bdk::{
    bitcoin::{Address, Network},
    wallet::{AddressIndex, Update},
    SignOptions, Wallet,
};
use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_file_store::Store;

const DB_MAGIC: &str = "bdk_wallet_esplora_async_example";
const SEND_AMOUNT: u64 = 5000;
const STOP_GAP: usize = 50;
const PARALLEL_REQUESTS: usize = 5;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let db_path = std::env::temp_dir().join("bdk-esplora-async-example");
    let db = Store::<bdk::wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";

    let mut wallet = Wallet::builder(external_descriptor)
        .with_change_descriptor(internal_descriptor)
        .with_network(Network::Testnet)
        .init_or_load(db)?;

    let address = wallet.try_get_address(AddressIndex::New)?;
    println!("Generated Address: {}", address);

    let balance = wallet.get_balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client =
        esplora_client::Builder::new("https://blockstream.info/testnet/api").build_async()?;

    let prev_tip = wallet.latest_checkpoint();
    let keychain_spks = wallet
        .spks_of_all_keychains()
        .into_iter()
        .map(|(k, k_spks)| {
            let mut once = Some(());
            let mut stdout = std::io::stdout();
            let k_spks = k_spks
                .inspect(move |(spk_i, _)| match once.take() {
                    Some(_) => print!("\nScanning keychain [{:?}]", k),
                    None => print!(" {:<3}", spk_i),
                })
                .inspect(move |_| stdout.flush().expect("must flush"));
            (k, k_spks)
        })
        .collect();
    let (update_graph, last_active_indices) = client
        .scan_txs_with_keychains(keychain_spks, None, None, STOP_GAP, PARALLEL_REQUESTS)
        .await?;
    let missing_heights = update_graph.missing_heights(wallet.local_chain());
    let chain_update = client.update_local_chain(prev_tip, missing_heights).await?;
    let update = Update {
        last_active_indices,
        graph: update_graph,
        chain: Some(chain_update),
    };
    wallet.apply_update(update)?;
    wallet.commit()?;
    println!();

    let balance = wallet.get_balance();
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

    let tx = psbt.extract_tx();
    client.broadcast(&tx).await?;
    println!("Tx broadcasted! Txid: {}", tx.txid());

    Ok(())
}
