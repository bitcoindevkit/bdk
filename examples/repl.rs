use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use rustyline::error::ReadlineError;
use rustyline::Editor;

use clap::AppSettings;

#[allow(unused_imports)]
use log::{debug, error, info, trace, LevelFilter};

use bitcoin::Network;

use magical_bitcoin_wallet::bitcoin;
use magical_bitcoin_wallet::blockchain::ElectrumBlockchain;
use magical_bitcoin_wallet::cli;
use magical_bitcoin_wallet::sled;
use magical_bitcoin_wallet::{Client, Wallet};

fn prepare_home_dir() -> PathBuf {
    let mut dir = PathBuf::new();
    dir.push(&dirs::home_dir().unwrap());
    dir.push(".magical-bitcoin");

    if !dir.exists() {
        info!("Creating home directory {}", dir.as_path().display());
        fs::create_dir(&dir).unwrap();
    }

    dir.push("database.sled");
    dir
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let app = cli::make_cli_subcommands();
    let mut repl_app = app.clone().setting(AppSettings::NoBinaryName);

    let app = cli::add_global_flags(app);

    let matches = app.get_matches();

    // TODO
    // let level = match matches.occurrences_of("v") {
    //     0 => LevelFilter::Info,
    //     1 => LevelFilter::Debug,
    //     _ => LevelFilter::Trace,
    // };

    let network = match matches.value_of("network") {
        Some("regtest") => Network::Regtest,
        Some("testnet") | _ => Network::Testnet,
    };

    let descriptor = matches.value_of("descriptor").unwrap();
    let change_descriptor = matches.value_of("change_descriptor");
    debug!("descriptors: {:?} {:?}", descriptor, change_descriptor);

    let database = sled::open(prepare_home_dir().to_str().unwrap()).unwrap();
    let tree = database
        .open_tree(matches.value_of("wallet").unwrap())
        .unwrap();
    debug!("database opened successfully");

    let client = Client::new(matches.value_of("server").unwrap())
        .await
        .unwrap();
    let wallet = Wallet::new(
        descriptor,
        change_descriptor,
        network,
        tree,
        ElectrumBlockchain::from(client),
    )
    .await
    .unwrap();
    let wallet = Arc::new(wallet);

    if let Some(_sub_matches) = matches.subcommand_matches("repl") {
        let mut rl = Editor::<()>::new();

        // if rl.load_history("history.txt").is_err() {
        //     println!("No previous history.");
        // }

        loop {
            let readline = rl.readline(">> ");
            match readline {
                Ok(line) => {
                    if line.trim() == "" {
                        continue;
                    }

                    rl.add_history_entry(line.as_str());
                    let matches = repl_app.get_matches_from_safe_borrow(line.split(" "));
                    if let Err(err) = matches {
                        println!("{}", err.message);
                        continue;
                    }

                    if let Some(s) = cli::handle_matches(&Arc::clone(&wallet), matches.unwrap())
                        .await
                        .unwrap()
                    {
                        println!("{}", s);
                    }
                }
                Err(ReadlineError::Interrupted) => continue,
                Err(ReadlineError::Eof) => break,
                Err(err) => {
                    println!("{:?}", err);
                    break;
                }
            }
        }

    // rl.save_history("history.txt").unwrap();
    } else {
        if let Some(s) = cli::handle_matches(&wallet, matches).await.unwrap() {
            println!("{}", s);
        }
    }
}
