extern crate bitcoin;
extern crate clap;
extern crate log;
extern crate magical_bitcoin_wallet;
extern crate miniscript;
extern crate serde_json;

use std::str::FromStr;

use log::info;

use clap::{App, Arg};

use bitcoin::Network;
use miniscript::policy::Concrete;
use miniscript::Descriptor;

use magical_bitcoin_wallet::database::memory::MemoryDatabase;
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

    let descriptor = match matches.value_of("TYPE").unwrap() {
        "sh" => Descriptor::Sh(policy.compile().unwrap()),
        "wsh" => Descriptor::Wsh(policy.compile().unwrap()),
        "sh-wsh" => Descriptor::ShWsh(policy.compile().unwrap()),
        _ => panic!("Invalid type"),
    };

    info!("... Descriptor: {}", descriptor);

    let database = MemoryDatabase::new();

    let network = match matches.value_of("network") {
        Some("regtest") => Network::Regtest,
        Some("testnet") | _ => Network::Testnet,
    };
    let wallet: OfflineWallet<_> =
        Wallet::new_offline(&format!("{}", descriptor), None, network, database).unwrap();

    info!("... First address: {}", wallet.get_new_address().unwrap());

    if matches.is_present("parsed_policy") {
        let spending_policy = wallet.policies(ScriptType::External).unwrap();
        info!(
            "... Spending policy:\n{}",
            serde_json::to_string_pretty(&spending_policy).unwrap()
        );
    }
}
