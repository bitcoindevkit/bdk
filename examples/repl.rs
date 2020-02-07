extern crate base64;
extern crate clap;
extern crate dirs;
extern crate env_logger;
extern crate log;
extern crate magical_bitcoin_wallet;
extern crate rustyline;

use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};

use rustyline::error::ReadlineError;
use rustyline::Editor;

#[allow(unused_imports)]
use log::{debug, error, info, trace, LevelFilter};

use bitcoin::consensus::encode::{deserialize, serialize, serialize_hex};
use bitcoin::util::psbt::PartiallySignedTransaction;
use bitcoin::{Address, Network, OutPoint};

use magical_bitcoin_wallet::bitcoin;
use magical_bitcoin_wallet::sled;
use magical_bitcoin_wallet::types::ScriptType;
use magical_bitcoin_wallet::{Client, ExtendedDescriptor, Wallet};

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

fn parse_addressee(s: &str) -> Result<(Address, u64), String> {
    let parts: Vec<_> = s.split(":").collect();
    if parts.len() != 2 {
        return Err("Invalid format".to_string());
    }

    let addr = Address::from_str(&parts[0]);
    if let Err(e) = addr {
        return Err(format!("{:?}", e));
    }
    let val = u64::from_str(&parts[1]);
    if let Err(e) = val {
        return Err(format!("{:?}", e));
    }

    Ok((addr.unwrap(), val.unwrap()))
}

fn parse_outpoint(s: &str) -> Result<OutPoint, String> {
    OutPoint::from_str(s).map_err(|e| format!("{:?}", e))
}

fn addressee_validator(s: String) -> Result<(), String> {
    parse_addressee(&s).map(|_| ())
}

fn outpoint_validator(s: String) -> Result<(), String> {
    parse_outpoint(&s).map(|_| ())
}

fn main() {
    env_logger::init();

    let app = App::new("Magical Bitcoin Wallet")
        .version(option_env!("CARGO_PKG_VERSION").unwrap_or("unknown"))
        .author(option_env!("CARGO_PKG_AUTHORS").unwrap_or(""))
        .about("A modern, lightweight, descriptor-based wallet")
        .subcommand(
            SubCommand::with_name("get_new_address").about("Generates a new external address"),
        )
        .subcommand(SubCommand::with_name("sync").about("Syncs with the chosen Electrum server"))
        .subcommand(
            SubCommand::with_name("list_unspent").about("Lists the available spendable UTXOs"),
        )
        .subcommand(
            SubCommand::with_name("get_balance").about("Returns the current wallet balance"),
        )
        .subcommand(
            SubCommand::with_name("create_tx")
                .about("Creates a new unsigned tranasaction")
                .arg(
                    Arg::with_name("to")
                        .long("to")
                        .value_name("ADDRESS:SAT")
                        .help("Adds an addressee to the transaction")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(true)
                        .multiple(true)
                        .validator(addressee_validator),
                )
                .arg(
                    Arg::with_name("send_all")
                        .short("all")
                        .long("send_all")
                        .help("Sends all the funds (or all the selected utxos). Requires only one addressees of value 0"),
                )
                .arg(
                    Arg::with_name("utxos")
                        .long("utxos")
                        .value_name("TXID:VOUT")
                        .help("Selects which utxos *must* be spent")
                        .takes_value(true)
                        .number_of_values(1)
                        .multiple(true)
                        .validator(outpoint_validator),
                )
                .arg(
                    Arg::with_name("unspendable")
                        .long("unspendable")
                        .value_name("TXID:VOUT")
                        .help("Marks an utxo as unspendable")
                        .takes_value(true)
                        .number_of_values(1)
                        .multiple(true)
                        .validator(outpoint_validator),
                )
                .arg(
                    Arg::with_name("fee_rate")
                        .short("fee")
                        .long("fee_rate")
                        .value_name("SATS_VBYTE")
                        .help("Fee rate to use in sat/vbyte")
                        .takes_value(true),
                )
                .arg(
                    Arg::with_name("policy")
                        .long("policy")
                        .value_name("POLICY")
                        .help("Selects which policy will be used to satisfy the descriptor")
                        .takes_value(true)
                        .number_of_values(1),
                ),
        )
        .subcommand(
            SubCommand::with_name("policies")
                .about("Returns the available spending policies for the descriptor")
            )
        .subcommand(
            SubCommand::with_name("sign")
                .about("Signs and tries to finalize a PSBT")
                .arg(
                    Arg::with_name("psbt")
                        .long("psbt")
                        .value_name("BASE64_PSBT")
                        .help("Sets the PSBT to sign")
                        .takes_value(true)
                        .number_of_values(1)
                        .required(true),
                ));

    let mut repl_app = app.clone().setting(AppSettings::NoBinaryName);

    let app = app
        .arg(
            Arg::with_name("network")
                .short("n")
                .long("network")
                .value_name("NETWORK")
                .help("Sets the network")
                .takes_value(true)
                .default_value("testnet")
                .possible_values(&["testnet", "regtest"]),
        )
        .arg(
            Arg::with_name("wallet")
                .short("w")
                .long("wallet")
                .value_name("WALLET_NAME")
                .help("Selects the wallet to use")
                .takes_value(true)
                .default_value("main"),
        )
        .arg(
            Arg::with_name("server")
                .short("s")
                .long("server")
                .value_name("SERVER:PORT")
                .help("Sets the Electrum server to use")
                .takes_value(true)
                .default_value("tn.not.fyi:55001"),
        )
        .arg(
            Arg::with_name("descriptor")
                .short("d")
                .long("descriptor")
                .value_name("DESCRIPTOR")
                .help("Sets the descriptor to use for the external addresses")
                .required(true)
                .takes_value(true),
        )
        .arg(
            Arg::with_name("change_descriptor")
                .short("c")
                .long("change_descriptor")
                .value_name("DESCRIPTOR")
                .help("Sets the descriptor to use for internal addresses")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        )
        .subcommand(SubCommand::with_name("repl").about("Opens an interactive shell"));

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

    let descriptor = matches
        .value_of("descriptor")
        .map(|x| ExtendedDescriptor::from_str(x).unwrap())
        .unwrap();
    let change_descriptor = matches
        .value_of("change_descriptor")
        .map(|x| ExtendedDescriptor::from_str(x).unwrap());
    debug!("descriptors: {:?} {:?}", descriptor, change_descriptor);

    let database = sled::open(prepare_home_dir().to_str().unwrap()).unwrap();
    let tree = database
        .open_tree(matches.value_of("wallet").unwrap())
        .unwrap();
    debug!("database opened successfully");

    let client = Client::new(matches.value_of("server").unwrap()).unwrap();
    let wallet = Wallet::new(descriptor, change_descriptor, network, tree, client);

    // TODO: print errors in a nice way
    let handle_matches = |matches: ArgMatches<'_>| {
        if let Some(_sub_matches) = matches.subcommand_matches("get_new_address") {
            println!("{}", wallet.get_new_address().unwrap().to_string());
        } else if let Some(_sub_matches) = matches.subcommand_matches("sync") {
            wallet.sync(None, None).unwrap();
        } else if let Some(_sub_matches) = matches.subcommand_matches("list_unspent") {
            for utxo in wallet.list_unspent().unwrap() {
                println!("{} value {} SAT", utxo.outpoint, utxo.txout.value);
            }
        } else if let Some(_sub_matches) = matches.subcommand_matches("get_balance") {
            println!("{} SAT", wallet.get_balance().unwrap());
        } else if let Some(sub_matches) = matches.subcommand_matches("create_tx") {
            let addressees = sub_matches
                .values_of("to")
                .unwrap()
                .map(|s| parse_addressee(s).unwrap())
                .collect();
            let send_all = sub_matches.is_present("send_all");
            let fee_rate = sub_matches
                .value_of("fee_rate")
                .map(|s| f32::from_str(s).unwrap())
                .unwrap_or(1.0);
            let utxos = sub_matches
                .values_of("utxos")
                .map(|s| s.map(|i| parse_outpoint(i).unwrap()).collect());
            let unspendable = sub_matches
                .values_of("unspendable")
                .map(|s| s.map(|i| parse_outpoint(i).unwrap()).collect());
            let policy: Option<Vec<_>> = sub_matches
                .value_of("policy")
                .map(|s| serde_json::from_str::<Vec<Vec<usize>>>(&s).unwrap());

            let result = wallet
                .create_tx(
                    addressees,
                    send_all,
                    fee_rate * 1e-5,
                    policy,
                    utxos,
                    unspendable,
                )
                .unwrap();
            println!("{:#?}", result.1);
            println!("PSBT: {}", base64::encode(&serialize(&result.0)));
        } else if let Some(_sub_matches) = matches.subcommand_matches("policies") {
            println!(
                "External: {}",
                serde_json::to_string(&wallet.policies(ScriptType::External).unwrap()).unwrap()
            );
            println!(
                "Internal: {}",
                serde_json::to_string(&wallet.policies(ScriptType::Internal).unwrap()).unwrap()
            );
        } else if let Some(sub_matches) = matches.subcommand_matches("sign") {
            let psbt = base64::decode(sub_matches.value_of("psbt").unwrap()).unwrap();
            let psbt: PartiallySignedTransaction = deserialize(&psbt).unwrap();
            let (psbt, finalized) = wallet.sign(psbt).unwrap();

            println!("Finalized: {}", finalized);
            if finalized {
                println!("Extracted: {}", serialize_hex(&psbt.extract_tx()));
            } else {
                println!("PSBT: {}", base64::encode(&serialize(&psbt)));
            }
        }
    };

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

                    handle_matches(matches.unwrap());
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
        handle_matches(matches);
    }
}
