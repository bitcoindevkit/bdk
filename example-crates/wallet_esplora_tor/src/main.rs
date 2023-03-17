use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::str::FromStr;
use std::time::Duration;
use std::{env, thread};

use libtor::{LogDestination, LogLevel, Tor, TorFlag};

use bdk::{
    bitcoin::{Address, Network},
    wallet::AddressIndex,
    SignOptions, Wallet,
};
use bdk_esplora::esplora_client;
use bdk_esplora::EsploraExt;
use bdk_file_store::KeychainStore;

const SEND_AMOUNT: u64 = 5000;
const STOP_GAP: usize = 50;
const PARALLEL_REQUESTS: usize = 5;

/// This will create a wallet from an xpriv and sync it by connecting to an Esplora server
/// over Tor network, using blocking calls with `ureq`.
///
/// This can be run with `cargo run -p bdk-esplora-wallet-tor-example`
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
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = std::env::temp_dir().join("bdk-esplora-tor-example");
    let db = KeychainStore::new_from_path(db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/1/*)";

    let mut wallet = Wallet::new(
        external_descriptor,
        Some(internal_descriptor),
        db,
        Network::Testnet,
    )?;

    let address = wallet.get_address(AddressIndex::New);
    println!("Generated Address: {}", address);

    let balance = wallet.get_balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    // Scanning the chain...
    let esplora_url =
        "http://explorerzydxu5ecjrkwceayqybizmpjjznk5izmitf2modhcusuqlid.onion/testnet/api";

    let socks_addr = start_tor();

    println!(
        "Connecting to {} via SOCKS5 proxy {}",
        &esplora_url, &socks_addr
    );

    let client_builder = esplora_client::Builder::new(esplora_url)
        .proxy(format!("socks5://{}", socks_addr).as_str())
        .timeout(120);

    let client = client_builder.build_blocking()?;
    let checkpoints = wallet.checkpoints();
    let spks = wallet
        .spks_of_all_keychains()
        .into_iter()
        .map(|(k, spks)| {
            let mut first = true;
            (
                k,
                spks.inspect(move |(spk_i, _)| {
                    if first {
                        first = false;
                        print!("\nScanning keychain [{:?}]:", k);
                    }
                    print!(" {}", spk_i);
                    let _ = std::io::stdout().flush();
                }),
            )
        })
        .collect();
    let update = client.scan(
        checkpoints,
        spks,
        core::iter::empty(),
        core::iter::empty(),
        STOP_GAP,
        PARALLEL_REQUESTS,
    )?;
    println!();
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

    let faucet_address = Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?;

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(faucet_address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let (mut psbt, _) = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx();
    client.broadcast(&tx)?;
    println!("Tx broadcasted! Txid: {}", tx.txid());

    Ok(())
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
