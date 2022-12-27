use std::fs::OpenOptions;
use std::io::Read;
use std::str::FromStr;
use std::time::Duration;
use std::{env, thread};

use bitcoin::{util::bip32, Network};
use esplora_client::Builder;
use libtor::{LogDestination, LogLevel, Tor, TorFlag};

use bdk::{
    blockchain::esplora::EsploraBlockchain, database::MemoryDatabase, template::Bip84,
    KeychainKind, SyncOptions, Wallet,
};
/// This will create a wallet from an xpriv and sync it by connecting to an Esplora server
/// over Tor network, using blocking calls with `ureq`.
///
/// This can be run with `cargo run --features="use-esplora-blocking" --example esplora_backend_with_tor`
/// in the root folder.
fn main() {
    let network = Network::Testnet;

    let xpriv_str =
        "tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy";

    let esplora_url =
        "http://explorerzydxu5ecjrkwceayqybizmpjjznk5izmitf2modhcusuqlid.onion/testnet/api";

    let xpriv = bip32::ExtendedPrivKey::from_str(xpriv_str).unwrap();

    let socks_addr = start_tor();

    let client = Builder::new(esplora_url)
        .proxy(&format!("socks5://{}", socks_addr))
        .timeout(120) // seconds
        .build_blocking()
        .unwrap();

    println!(
        "Connecting to {} via SOCKS5 proxy {}",
        &esplora_url, &socks_addr
    );

    let blockchain = EsploraBlockchain::from_client(client, 20);

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
