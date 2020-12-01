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
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::Network;
use clap::AppSettings;
use log::{debug, error, info, trace, warn, LevelFilter};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use structopt::StructOpt;

use bdk::bitcoin;
use bdk::blockchain::esplora::EsploraBlockchainConfig;
use bdk::blockchain::{
    AnyBlockchain, AnyBlockchainConfig, ConfigurableBlockchain, ElectrumBlockchainConfig,
};
use bdk::cli::{self, WalletOpt, WalletSubCommand};
use bdk::sled;
use bdk::Wallet;

#[derive(Debug, StructOpt, Clone, PartialEq)]
#[structopt(name = "BDK Wallet", setting = AppSettings::NoBinaryName,
version = option_env ! ("CARGO_PKG_VERSION").unwrap_or("unknown"),
author = option_env ! ("CARGO_PKG_AUTHORS").unwrap_or(""))]
struct ReplOpt {
    /// Wallet sub-command
    #[structopt(subcommand)]
    pub subcommand: WalletSubCommand,
}

fn prepare_home_dir() -> PathBuf {
    let mut dir = PathBuf::new();
    dir.push(&dirs_next::home_dir().unwrap());
    dir.push(".bdk-bitcoin");

    if !dir.exists() {
        info!("Creating home directory {}", dir.as_path().display());
        fs::create_dir(&dir).unwrap();
    }

    dir.push("database.sled");
    dir
}

fn main() {
    let cli_opt: WalletOpt = WalletOpt::from_args();

    let level = LevelFilter::from_str(cli_opt.log_level.as_str()).unwrap_or(LevelFilter::Info);
    env_logger::builder().filter_level(level).init();

    let network = Network::from_str(cli_opt.network.as_str()).unwrap_or(Network::Testnet);
    debug!("network: {:?}", network);
    if network == Network::Bitcoin {
        warn!("This is experimental software and not currently recommended for use on Bitcoin mainnet, proceed with caution.")
    }

    let descriptor = cli_opt.descriptor.as_str();
    let change_descriptor = cli_opt.change_descriptor.as_deref();
    debug!("descriptors: {:?} {:?}", descriptor, change_descriptor);

    let database = sled::open(prepare_home_dir().to_str().unwrap()).unwrap();
    let tree = database.open_tree(cli_opt.wallet).unwrap();
    debug!("database opened successfully");

    let config = match cli_opt.esplora {
        Some(base_url) => AnyBlockchainConfig::Esplora(EsploraBlockchainConfig {
            base_url: base_url.to_string(),
            concurrency: Some(cli_opt.esplora_concurrency),
        }),
        None => AnyBlockchainConfig::Electrum(ElectrumBlockchainConfig {
            url: cli_opt.electrum,
            socks5: cli_opt.proxy,
            retry: 10,
            timeout: 10,
        }),
    };

    let wallet = Wallet::new(
        descriptor,
        change_descriptor,
        network,
        tree,
        AnyBlockchain::from_config(&config).unwrap(),
    )
    .unwrap();

    let wallet = Arc::new(wallet);

    match cli_opt.subcommand {
        WalletSubCommand::Other(external) if external.contains(&"repl".to_string()) => {
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
                        let split_line: Vec<&str> = line.split(" ").collect();
                        let repl_subcommand: Result<ReplOpt, clap::Error> =
                            ReplOpt::from_iter_safe(split_line);
                        debug!("repl_subcommand = {:?}", repl_subcommand);

                        if let Err(err) = repl_subcommand {
                            println!("{}", err.message);
                            continue;
                        }

                        let result = cli::handle_wallet_subcommand(
                            &Arc::clone(&wallet),
                            repl_subcommand.unwrap().subcommand,
                        )
                        .unwrap();
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
        }
        _ => {
            let result = cli::handle_wallet_subcommand(&wallet, cli_opt.subcommand).unwrap();
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }
    }
}
