const DB_MAGIC: &str = "bdk_wallet_esplora_blocking_example";
const SEND_AMOUNT: u64 = 1000;
const STOP_GAP: usize = 5;
const PARALLEL_REQUESTS: usize = 1;

use env_logger::Env;
use log::info;
use std::env;
use std::str::FromStr;

use bdk::wallet::Update;
use bdk::{
    bitcoin::{Address, Network},
    wallet::AddressIndex,
    SignOptions, Wallet,
};
use bdk_esplora::{esplora_client, EsploraExt};
use bdk_file_store::Store;

fn main() -> Result<(), anyhow::Error> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1);

    let db_path = "bdk_wallet_esplora_blocking_example.dat";
    let db = Store::<bdk::wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
    let network = Network::Signet;

    let mut wallet =
        Wallet::new_or_load(external_descriptor, Some(internal_descriptor), db, network)?;

    let address = wallet.try_get_address(AddressIndex::New)?;
    info!("Generated Address: {}", address);

    let balance = wallet.get_balance();
    info!("Wallet balance: {} sats", balance.total());

    let client =
        esplora_client::Builder::new("http://signet.bitcoindevkit.net").build_blocking()?;

    let (update, cmd) = match cmd.map(|c| c.as_str()) {
        Some(cmd) if cmd == "fullscan" => {
            info!("Start full scan...");
            // 1. get data required to do a wallet full_scan
            let mut request = wallet.full_scan_request();
            request.inspect_spks(move |(i, s)| {
                info!(
                    "scanning index: {}, address: {}",
                    i,
                    Address::from_script(s, network).expect("address")
                );
            });
            // 2. full scan to discover wallet transactions
            let result = client.full_scan(request, STOP_GAP, PARALLEL_REQUESTS)?;
            // 3. create wallet update
            let update = Update {
                last_active_indices: result.last_active_indices,
                graph: result.graph_update,
                chain: Some(result.chain_update),
            };
            Ok((update, cmd))
        }
        Some(cmd) if cmd == "sync" => {
            info!("Start sync...");
            // 1. get data required to do a wallet sync, if also syncing previously used addresses set unused_spks_only = false
            let mut request = wallet.sync_revealed_spks_request();
            request.inspect_spks(move |index, spk| {
                info!(
                    "syncing address {}: {}",
                    index,
                    Address::from_script(spk, network).expect("address")
                );
            });
            // 2. sync unused wallet spks (addresses), unconfirmed tx, and utxos
            let result = client.sync(request, PARALLEL_REQUESTS)?;
            // 3. create wallet update
            let update = Update {
                graph: result.graph_update,
                chain: Some(result.chain_update),
                ..Update::default()
            };
            Ok((update, cmd))
        }
        _ => Err(()),
    }
    .expect("Specify if you want to do a wallet 'fullscan' or a 'sync'.");

    // 4. apply update to wallet
    wallet.apply_update(update)?;
    // 5. commit wallet update to database
    wallet.commit()?;

    let balance = wallet.get_balance();
    info!("Wallet balance after {}: {} sats", cmd, balance.total());

    if balance.total() < SEND_AMOUNT {
        info!(
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
    client.broadcast(&tx)?;
    info!("Tx broadcasted! Txid: {}", tx.txid());

    Ok(())
}
