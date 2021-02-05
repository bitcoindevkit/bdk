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
        .and_then(|n| Some(Network::from_str(n)))
        .transpose()
        .unwrap()
        .unwrap_or(Network::Testnet);
    let wallet = Wallet::new_offline(&format!("{}", descriptor), None, network, database)?;

    info!("... First address: {}", wallet.get_new_address()?);

    if matches.is_present("parsed_policy") {
        let spending_policy = wallet.policies(KeychainKind::External)?;
        info!(
            "... Spending policy:\n{}",
            serde_json::to_string_pretty(&spending_policy)?
        );
    }

    Ok(())
}
