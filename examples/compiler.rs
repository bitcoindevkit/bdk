extern crate bitcoin;
extern crate clap;
extern crate log;
extern crate magical_bitcoin_wallet;
extern crate miniscript;
extern crate rand;
extern crate serde_json;
extern crate sled;

use std::str::FromStr;

use log::info;

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

use clap::{App, Arg};

use bitcoin::Network;
use miniscript::policy::Concrete;
use miniscript::Descriptor;

use magical_bitcoin_wallet::types::ScriptType;
use magical_bitcoin_wallet::{OfflineWallet, Wallet};

fn main() {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let matches = App::new("Miniscript Compiler")
        .arg(
            Arg::with_name("POLICY")
                .help("Sets the spending policy to compile")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::with_name("TYPE")
                .help("Sets the script type used to embed the compiled policy")
                .required(true)
                .index(2)
                .possible_values(&["sh", "wsh", "sh-wsh"]),
        )
        .arg(
            Arg::with_name("parsed_policy")
                .long("parsed_policy")
                .short("p")
                .help("Also return the parsed spending policy in JSON format"),
        )
        .arg(
            Arg::with_name("network")
                .short("n")
                .long("network")
                .help("Sets the network")
                .takes_value(true)
                .default_value("testnet")
                .possible_values(&["testnet", "regtest"]),
        )
        .get_matches();

    let policy_str = matches.value_of("POLICY").unwrap();
    info!("Compiling policy: {}", policy_str);

    let policy = Concrete::<String>::from_str(&policy_str).unwrap();
    let compiled = policy.compile().unwrap();

    let descriptor = match matches.value_of("TYPE").unwrap() {
        "sh" => Descriptor::Sh(compiled),
        "wsh" => Descriptor::Wsh(compiled),
        "sh-wsh" => Descriptor::ShWsh(compiled),
        _ => panic!("Invalid type"),
    };

    info!("... Descriptor: {}", descriptor);

    let temp_db = {
        let mut temp_db = std::env::temp_dir();
        let rand_string: String = thread_rng().sample_iter(&Alphanumeric).take(15).collect();
        temp_db.push(rand_string);

        let database = sled::open(&temp_db).unwrap();

        let network = match matches.value_of("network") {
            Some("regtest") => Network::Regtest,
            Some("testnet") | _ => Network::Testnet,
        };
        let wallet: OfflineWallet<_> = Wallet::new_offline(
            &format!("{}", descriptor),
            None,
            network,
            database.open_tree("").unwrap(),
        )
        .unwrap();

        info!("... First address: {}", wallet.get_new_address().unwrap());

        if matches.is_present("parsed_policy") {
            let spending_policy = wallet.policies(ScriptType::External).unwrap();
            info!(
                "... Spending policy:\n{}",
                serde_json::to_string_pretty(&spending_policy).unwrap()
            );
        }

        temp_db
    };

    std::fs::remove_dir_all(temp_db).unwrap();
}
