// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use rustyline::error::ReadlineError;
use rustyline::Editor;

use clap::AppSettings;

#[allow(unused_imports)]
use log::{debug, error, info, trace, LevelFilter};

use bitcoin::Network;

use bdk::bitcoin;
use bdk::blockchain::{
    AnyBlockchain, AnyBlockchainConfig, ConfigurableBlockchain, ElectrumBlockchainConfig,
};
use bdk::cli;
use bdk::sled;
use bdk::Wallet;

use bdk::blockchain::esplora::EsploraBlockchainConfig;

fn prepare_home_dir() -> PathBuf {
    let mut dir = PathBuf::new();
    dir.push(&dirs::home_dir().unwrap());
    dir.push(".bdk-bitcoin");

    if !dir.exists() {
        info!("Creating home directory {}", dir.as_path().display());
        fs::create_dir(&dir).unwrap();
    }

    dir.push("database.sled");
    dir
}

fn main() {
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

    let config = match matches.value_of("esplora") {
        Some(base_url) => AnyBlockchainConfig::Esplora(EsploraBlockchainConfig {
            base_url: base_url.to_string(),
            concurrency: matches
                .value_of("esplora_concurrency")
                .and_then(|v| v.parse::<u8>().ok()),
        }),
        None => AnyBlockchainConfig::Electrum(ElectrumBlockchainConfig {
            url: matches.value_of("server").unwrap().to_string(),
            socks5: matches.value_of("proxy").map(ToString::to_string),
        }),
    };
    let wallet = Arc::new(
        Wallet::new(
            descriptor,
            change_descriptor,
            network,
            tree,
            AnyBlockchain::from_config(&config).unwrap(),
        )
        .unwrap(),
    );

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

                    let result =
                        cli::handle_matches(&Arc::clone(&wallet), matches.unwrap()).unwrap();
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
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
        let result = cli::handle_matches(&wallet, matches).unwrap();
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    }
}
