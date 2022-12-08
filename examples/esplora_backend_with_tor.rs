use std::fs::File;
use std::io::Read;
use std::str::FromStr;
use std::time::Duration;
use std::{env, thread};

use bdk::blockchain::Blockchain;
use bdk::{
    blockchain::esplora::EsploraBlockchain,
    database::MemoryDatabase,
    template::Bip84,
    wallet::{export::FullyNodedExport, AddressIndex},
    KeychainKind, SyncOptions, Wallet,
};
use bitcoin::{
    util::bip32::{self, ExtendedPrivKey},
    Network,
};
use esplora_client::Builder;
use libtor::{LogDestination, LogLevel, Tor, TorFlag};

pub mod utils;

use crate::utils::tx::build_signed_tx;

/// This will create a wallet from an xpriv and get the balance by connecting to an Esplora server
/// over Tor network, using blocking calls with `ureq`.
/// If enough amount is available, this will send a transaction to an address.
/// Otherwise, this will display a wallet address to receive funds.
///
/// This can be run with `cargo run --features=use-esplora-ureq --example esplora_backend_synchronous`
/// in the root folder.
fn main() {
    let network = Network::Signet;

    let xpriv = "tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy";

    let esplora_url =
        "http://explorerzydxu5ecjrkwceayqybizmpjjznk5izmitf2modhcusuqlid.onion/testnet/api";

    run(&network, esplora_url, xpriv);
}

fn create_wallet(network: &Network, xpriv: &ExtendedPrivKey) -> Wallet<MemoryDatabase> {
    Wallet::new(
        Bip84(*xpriv, KeychainKind::External),
        Some(Bip84(*xpriv, KeychainKind::Internal)),
        *network,
        MemoryDatabase::default(),
    )
    .unwrap()
}

fn run(network: &Network, esplora_url: &str, xpriv: &str) {
    let xpriv = bip32::ExtendedPrivKey::from_str(xpriv).unwrap();

    let socks_addr = start_tor();

    let client = Builder::new(esplora_url)
        .proxy(&format!("socks5://{}", socks_addr))
        .timeout(120) // seconds
        .build_blocking()
        .expect("Should never fail with no proxy and timeout");

    println!(
        "Connecting to {} via SOCKS5 proxy {}",
        &esplora_url, &socks_addr
    );

    let blockchain = EsploraBlockchain::from_client(client, 20);

    let wallet = create_wallet(network, &xpriv);

    wallet.sync(&blockchain, SyncOptions::default()).unwrap();

    let address = wallet.get_address(AddressIndex::New).unwrap().address;

    println!("address: {}", address);

    let balance = wallet.get_balance().unwrap();

    println!("Available coins in BDK wallet : {} sats", balance);

    if balance.confirmed > 10500 {
        // the wallet sends the amount to itself.
        let recipient_address = wallet
            .get_address(AddressIndex::New)
            .unwrap()
            .address
            .to_string();

        let amount = 9359;

        let tx = build_signed_tx(&wallet, &recipient_address, amount);

        blockchain.broadcast(&tx).unwrap();

        println!("tx id: {}", tx.txid());
    } else {
        println!("Insufficient Funds. Fund the wallet with the address above");
    }

    let export = FullyNodedExport::export_wallet(&wallet, "exported wallet", true)
        .map_err(ToString::to_string)
        .map_err(bdk::Error::Generic)
        .unwrap();

    println!("------\nWallet Backup: {}", export.to_string());
}

pub fn start_tor() -> String {
    let socks_port = 19050;

    let data_dir = format!("{}/{}", env::temp_dir().display(), "bdk-tor-example");
    let log_file_name = format!("{}/{}", &data_dir, "log");

    println!("Staring Tor in {}", &data_dir);

    truncate_log(&log_file_name);

    Tor::new()
        .flag(TorFlag::DataDirectory(data_dir.into()))
        .flag(TorFlag::LogTo(
            LogLevel::Notice,
            LogDestination::File(log_file_name.as_str().into()),
        ))
        .flag(TorFlag::SocksPort(socks_port))
        .flag(TorFlag::Custom("ExitPolicy reject *:*".into()))
        .flag(TorFlag::Custom("BridgeRelay 0".into()))
        .start_background();

    let mut started = false;
    let mut tries = 0;
    while !started {
        tries += 1;
        if tries > 120 {
            panic!(
                "It took too long to start Tor. See {} for details",
                &log_file_name
            );
        }

        thread::sleep(Duration::from_millis(1000));
        started = find_string_in_log(&log_file_name, &"Bootstrapped 100%".into());
    }

    println!("Tor started");

    format!("127.0.0.1:{}", socks_port)
}

fn truncate_log(filename: &String) {
    let path = std::path::Path::new(filename);
    if path.exists() {
        let file = File::options()
            .write(true)
            .open(path)
            .expect("no such file");
        file.set_len(0).expect("cannot set size to 0");
    }
}

fn find_string_in_log(filename: &String, str: &String) -> bool {
    let path = std::path::Path::new(filename);
    if path.exists() {
        let mut file = File::open(path).expect("cannot open log file");
        let mut buf = String::new();
        file.read_to_string(&mut buf).expect("cannot read log file");
        buf.contains(str)
    } else {
        false
    }
}
