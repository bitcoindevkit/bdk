#![allow(unused)]
use std::{collections::BTreeSet, io::Write};

use anyhow::Ok;
use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_wallet::{
    bitcoin::{Amount, Network, ScriptBuf},
    rusqlite::Connection,
    KeychainKind, SignOptions, Wallet,
};

use bdk_wallet::test_utils::*;

const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 5;
const PARALLEL_REQUESTS: usize = 1;

const DB_PATH: &str = ".bdk-example-esplora-async.sqlite";
const NETWORK: Network = Network::Signet;
const EXTERNAL_DESC: &str = "tr(tprv8ZgxMBicQKsPczwfSDDHGpmNeWzKaMajLtkkNUdBoisixK3sW3YTC8subMCsTJB7sM4kaJJ7K1cNVM37aZoJ7dMBt2HRYLQzoFPqPMC8cTr/86'/1'/0'/0/*)";
const INTERNAL_DESC: &str = "tr(tprv8ZgxMBicQKsPczwfSDDHGpmNeWzKaMajLtkkNUdBoisixK3sW3YTC8subMCsTJB7sM4kaJJ7K1cNVM37aZoJ7dMBt2HRYLQzoFPqPMC8cTr/86'/1'/0'/1/*)";

const ESPLORA_URL: &str = "http://signet.bitcoindevkit.net";

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let mut conn = Connection::open(DB_PATH)?;

    let wallet_opt = Wallet::load()
        .descriptor(KeychainKind::External, Some(EXTERNAL_DESC))
        .descriptor(KeychainKind::Internal, Some(INTERNAL_DESC))
        .extract_keys()
        .check_network(NETWORK)
        .load_wallet(&mut conn)?;
    let mut wallet = match wallet_opt {
        Some(wallet) => wallet,
        None => Wallet::create(EXTERNAL_DESC, INTERNAL_DESC)
            .network(NETWORK)
            .create_wallet(&mut conn)?,
    };

    // Sync
    let client = esplora_client::Builder::new(ESPLORA_URL).build_async()?;
    let request = wallet.start_full_scan().inspect({
        let mut stdout = std::io::stdout();
        let mut once = BTreeSet::<KeychainKind>::new();
        move |keychain, spk_i, _| {
            if once.insert(keychain) {
                print!("\nScanning keychain [{:?}]", keychain);
            }
            print!(" {:<3}", spk_i);
            stdout.flush().unwrap();
        }
    });
    let update = client
        .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
        .await?;
    wallet.apply_update(update)?;
    wallet.persist(&mut conn)?;
    println!();
    let old_balance = wallet.balance();
    println!("Balance after sync: {}", old_balance.total());

    // Build tx that sends all to a foreign recipient
    let recip = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;
    let mut b = wallet.build_tx();
    b.drain_to(recip).drain_wallet();
    let mut psbt = b.finish()?;
    assert!(wallet.sign(&mut psbt, SignOptions::default())?);
    let tx = psbt.extract_tx()?;
    let txid = tx.compute_txid();
    insert_tx(&mut wallet, tx.clone());
    wallet.persist(&mut conn)?;

    println!("Balance after send: {}", wallet.balance().total());
    assert_eq!(wallet.balance().total(), Amount::ZERO);

    // Cancel spend
    wallet.cancel_tx(&tx);
    assert_eq!(wallet.balance(), old_balance);
    println!("Balance after cancel: {}", wallet.balance().total());
    wallet.persist(&mut conn)?;

    // Load one more time
    wallet = Wallet::load()
        .load_wallet(&mut conn)?
        .expect("must load existing");

    assert_eq!(
        wallet.balance(),
        old_balance,
        "balance did not match original"
    );

    Ok(())
}
