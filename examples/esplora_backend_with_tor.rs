use std::fs::OpenOptions;
use std::io::Read;
use std::str::FromStr;
use std::time::Duration;
use std::{env, thread};

use bitcoin::{util::bip32, Network};
use libtor::{LogDestination, LogLevel, Tor, TorFlag};

use bdk::blockchain::esplora::EsploraBlockchain;
use bdk::blockchain::esplora::EsploraBlockchainConfig;
use bdk::blockchain::ConfigurableBlockchain;
use bdk::database::MemoryDatabase;
use bdk::template::Bip84;
use bdk::wallet::AddressIndex;
use bdk::{KeychainKind, SyncOptions, Wallet};

use crate::utils::tx::build_signed_tx;

pub mod utils;

/// This will create a wallet from an xpriv and sync it by connecting to an Esplora server
/// over Tor network, using blocking calls with `ureq`.
///
/// This can be run with `cargo run --features="use-esplora-blocking libtor" --example esplora_backend_with_tor`
/// in the root folder.
///
/// Note, that libtor requires a C compiler and C build tools.
///
/// Here is how to install them on Ubuntu:
/// ```
/// sudo apt install autoconf automake clang file libtool openssl pkg-config
/// ```
/// And here is how to install them on Mac OS X:
/// ```
/// xcode-select --install
/// brew install autoconf
/// brew install automake
/// brew install libtool
/// brew install openssl
/// brew install pkg-config
/// export LDFLAGS="-L/opt/homebrew/opt/openssl/lib"
/// export CPPFLAGS="-I/opt/homebrew/opt/openssl/include"
/// ```
fn main() {
    let network = Network::Testnet;

    let xpriv_str =
        "tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy";

    let esplora_url =
        "http://explorerzydxu5ecjrkwceayqybizmpjjznk5izmitf2modhcusuqlid.onion/testnet/api";

    let xpriv = bip32::ExtendedPrivKey::from_str(xpriv_str).unwrap();

    let socks_addr = start_tor();

    println!(
        "Connecting to {} via SOCKS5 proxy {}",
        &esplora_url, &socks_addr
    );

    let esplora_config = EsploraBlockchainConfig {
        base_url: esplora_url.into(),
        concurrency: None,
        proxy: Some(format!("socks5://{}", socks_addr)),
        stop_gap: 20,
        timeout: Some(120),
    };

    let blockchain = EsploraBlockchain::from_config(&esplora_config).unwrap();

    let wallet = Wallet::new(
        Bip84(xpriv, KeychainKind::External),
        Some(Bip84(xpriv, KeychainKind::Internal)),
        network,
        MemoryDatabase::default(),
    )
    .unwrap();

    println!("Syncing the wallet");

    wallet.sync(&blockchain, SyncOptions::default()).unwrap();

    println!("Done!");

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
}

pub fn start_tor() -> String {
    let socks_port = 19050;

    let data_dir = format!("{}/{}", env::temp_dir().display(), "bdk-tor-example");
    let log_file_name = format!("{}/{}", &data_dir, "log");

    println!("Staring Tor in {}", &data_dir);

    truncate_log(&log_file_name);

    Tor::new()
        .flag(TorFlag::DataDirectory(data_dir))
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
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("no such file");
        file.set_len(0).expect("cannot set size to 0");
    }
}

fn find_string_in_log(filename: &String, str: &String) -> bool {
    let path = std::path::Path::new(filename);
    if path.exists() {
        let mut file = OpenOptions::new()
            .read(true)
            .open(path)
            .expect("no such file");
        let mut buf = String::new();
        file.read_to_string(&mut buf).expect("cannot read log file");
        buf.contains(str)
    } else {
        false
    }
}
