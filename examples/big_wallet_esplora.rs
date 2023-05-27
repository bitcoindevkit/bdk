use bitcoin::{Amount, OutPoint};
use std::borrow::BorrowMut;
use std::ops::{Div, Rem};
use std::str::FromStr;
use std::time::SystemTime;
use std::{thread, time};

use bdk::bitcoin::util::bip32::ExtendedPrivKey;
use bdk::bitcoin::Network;
use bdk::blockchain::{
    Blockchain, ConfigurableBlockchain, EsploraBlockchain, GetHeight, WalletSync,
};
use bdk::database::{AnyDatabase, AnyDatabaseConfig, ConfigurableDatabase, Database};
use bdk::template::Bip84;
use bdk::{KeychainKind, SignOptions, SyncOptions, Wallet};

use bdk::blockchain::esplora::EsploraBlockchainConfig;
use bdk::database::any::SqliteDbConfiguration;
use bdk::wallet::tx_builder::ChangeSpendPolicy;
use bdk::wallet::AddressIndex;
use bdk::KeychainKind::External;
use electrsd::bitcoind;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use electrsd::bitcoind::BitcoinD;
use futures::StreamExt;
use miniscript::psbt::PsbtExt;

/// This example will create a wallet from an xpriv and get the balance by starting and connecting
/// to local regtest bitcoind and electrs servers. It will tell bitcoind to send transactions to
/// addresses it controls up to index 200,000 and then sync and confirm the wallet balance.
///
/// This example is run from the root folder with:
/// ```
/// cargo run --example big_wallet_esplora --features="all-keys sqlite use-esplora-async reqwest-default-tls"
/// ```
fn main() {
    env_logger::init();

    let network = Network::Signet;
    //let xpriv = ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
    //let xpriv = ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPf3jyLFqfFunqDeTz6JEV5YngSVgWnvDeSzh1iCGi5gdWbgRtntGQL2HevsY51XJtZQeFeGeGg4TrZVrZHmLStLvjntJ8QaM").unwrap();
    let xpriv = ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPd16S7pZnjWam27aJq6eJLGms7nKKF5z5xLycwy1T7Ft1Bb1oXSXLfT43X2426seSxiT6g9eEv4sst457RP3yF3gQsejKYfu").unwrap();
    let descriptor = Bip84(xpriv.clone(), KeychainKind::External);
    let change_descriptor = Some(Bip84(xpriv, KeychainKind::Internal));

    let database_config = AnyDatabaseConfig::Sqlite(SqliteDbConfiguration {
        path: "./big_wallet_esplora.db".to_string(),
    });
    let database = AnyDatabase::from_config(&database_config).unwrap();

    let esplora_config = EsploraBlockchainConfig {
        base_url: "https://mutinynet.com/api".to_string(),
        proxy: None,
        concurrency: Some(250),
        stop_gap: 150,
        timeout: None,
    };
    let esplora_client = EsploraBlockchain::from_config(&esplora_config).unwrap();
    let wallet = Wallet::new(descriptor, change_descriptor, network, database).unwrap();

    println!("Initial BDK wallet syncing...");
    sync_wallet(&wallet, &esplora_client);

    let last_index = wallet
        .database()
        .get_last_index(External)
        .unwrap()
        .unwrap_or(0);
    println!("BDK wallet last external keychain index is: {}", last_index);

    let confirmed_balance = wallet.get_balance().unwrap().confirmed;
    let required_balance = 1_000_000;
    if required_balance > confirmed_balance {
        let address = wallet.get_address(AddressIndex::New).unwrap();
        println!(
            "Your confirmed balance is {} but {} SATs are required.",
            confirmed_balance, required_balance
        );
        println!(
            "Deposit {} SATs to: {}",
            required_balance - confirmed_balance,
            address
        );
        println!("Mutinynet faucet: https://faucet.mutinynet.com/");
        return;
    }

    let mut tx_builder = wallet.build_tx();

    for index in last_index..=200_000 {
        // every 100 addresses create add a new output to current transaction builder
        if index > 0 && index % 100 == 0 {
            let address = wallet
                .get_address(AddressIndex::Reset(index))
                .unwrap()
                .address;
            tx_builder.add_recipient(address.script_pubkey(), 1_000);
        }
        // every 200 addresses broadcast the transaction and create a new transaction builder
        if index > 0 && index % 20_000 == 0 {
            let (mut psbt, details) = tx_builder.finish().unwrap();
            wallet.sign(&mut psbt, SignOptions::default()).unwrap();
            println!(
                "Created tx paying address indexes {} to {}, every 100 indexes: https://mutinynet.com/tx/{}",
                index-20_000,
                index,
                details.txid
            );

            let tx = &psbt.clone().extract_tx();
            println!("Broadcasting tx.");
            let result = esplora_client.broadcast(tx);

            if result.is_err() {
                use bitcoin::consensus::serialize;
                use bitcoin::hashes::hex::ToHex;

                println!("Broadcast failed!");
                dbg!(&tx);
                println!("tx: {}", serialize(tx).to_hex());
                panic!();
            };

            print!("Waiting for next block");
            let current_height = esplora_client.get_height().unwrap();
            let mut new_height = current_height;
            while new_height <= current_height {
                let ten_second = time::Duration::from_secs(10);
                thread::sleep(ten_second);
                print!(".");
                new_height = esplora_client.get_height().unwrap();
            }
            println!("Wallet syncing...");
            sync_wallet(&wallet, &esplora_client);
            tx_builder = wallet.build_tx();
        }
    }
    let balance = wallet.get_balance().unwrap();
    println!("balance = {}", &balance);
}

fn sync_wallet<B: WalletSync + GetHeight>(wallet: &Wallet<AnyDatabase>, esplora_client: &B) {
    let start = SystemTime::now();
    let result = wallet.sync(esplora_client, SyncOptions::default());
    if result.is_err() {
        dbg!(result);
        panic!();
    }
    let end = SystemTime::now();
    let duration = end.duration_since(start).unwrap();
    println!(
        "Wallet sync took {} minutes and {} seconds",
        duration.as_secs().div(60),
        duration.as_secs().rem(60)
    );
}
