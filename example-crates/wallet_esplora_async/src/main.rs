use std::str::FromStr;

use bdk::chain::spk_client::FullScanRequest;
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
    //let db_path = std::env::temp_dir().join("bdk-esplora-async-example");
    let db_path = "bdk-esplora-async-example";
    let db = Store::<bdk::wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
    let network = Network::Signet;

    let mut wallet =
        Wallet::new_or_load(external_descriptor, Some(internal_descriptor), db, network)?;

    let address = wallet.try_get_address(AddressIndex::New)?;
    println!("Generated Address: {}", address);

    let balance = wallet.get_balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    let client = esplora_client::Builder::new("http://signet.bitcoindevkit.net").build_async()?;

    let prev_tip = wallet.latest_checkpoint();
    let keychain_spks = wallet.all_unbounded_spk_iters();

    let mut request = FullScanRequest::new(prev_tip);
    request.add_spks_by_keychain(keychain_spks);
    request.inspect_spks(move |k, i, spk| {
        println!(
            "{:?}[{}]: {}",
            k,
            i,
            Address::from_script(spk, network).unwrap()
        );
    });
    println!("Scanning... ");
    let result = client
        .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
        .await?;
    println!("done. ");
    let update = Update {
        last_active_indices: result.last_active_indices,
        graph: result.graph_update,
        chain: Some(result.chain_update),
    };
    wallet.apply_update(update)?;
    wallet.commit()?;

    let balance = wallet.get_balance();
    println!("Wallet balance after syncing: {} sats", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} sats to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let faucet_address =
        Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?.require_network(network)?;

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
