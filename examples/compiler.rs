// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

extern crate bdk;
extern crate bitcoin;
extern crate clap;
extern crate log;
extern crate miniscript;
extern crate serde_json;

use std::error::Error;
use std::str::FromStr;

use log::info;

use clap::{App, Arg};

use bitcoin::Network;
use miniscript::policy::Concrete;
use miniscript::Descriptor;

use bdk::database::memory::MemoryDatabase;
use bdk::wallet::AddressIndex::New;
use bdk::{KeychainKind, Wallet};

fn main() -> Result<(), Box<dyn Error>> {
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
                .possible_values(&["testnet", "regtest", "bitcoin", "signet"]),
        )
        .get_matches();

    let policy_str = matches.value_of("POLICY").unwrap();
    info!("Compiling policy: {}", policy_str);

    let policy = Concrete::<String>::from_str(&policy_str)?;

    let descriptor = match matches.value_of("TYPE").unwrap() {
        "sh" => Descriptor::new_sh(policy.compile()?)?,
        "wsh" => Descriptor::new_wsh(policy.compile()?)?,
        "sh-wsh" => Descriptor::new_sh_wsh(policy.compile()?)?,
        _ => panic!("Invalid type"),
    };

    info!("... Descriptor: {}", descriptor);

    let database = MemoryDatabase::new();

    let network = matches
        .value_of("network")
        .map(|n| Network::from_str(n))
        .transpose()
        .unwrap()
        .unwrap_or(Network::Testnet);
    let wallet = Wallet::new_offline(&format!("{}", descriptor), None, network, database)?;

    info!("... First address: {}", wallet.get_address(New)?);

    if matches.is_present("parsed_policy") {
        let spending_policy = wallet.policies(KeychainKind::External)?;
        info!(
            "... Spending policy:\n{}",
            serde_json::to_string_pretty(&spending_policy)?
        );
    }

    Ok(())
}
