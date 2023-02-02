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
extern crate log;
extern crate miniscript;
extern crate serde_json;

use std::error::Error;
use std::str::FromStr;

use log::info;

use bitcoin::Network;
use miniscript::policy::Concrete;
use miniscript::Descriptor;

use bdk::database::memory::MemoryDatabase;
use bdk::wallet::AddressIndex::New;
use bdk::{KeychainKind, Wallet};

/// Miniscript policy is a high level abstraction of spending conditions. Defined in the
/// rust-miniscript library here  https://docs.rs/miniscript/7.0.0/miniscript/policy/index.html
/// rust-miniscript provides a `compile()` function that can be used to compile any miniscript policy
/// into a descriptor. This descriptor then in turn can be used in bdk a fully functioning wallet
/// can be derived from the policy.
///
/// This example demonstrates the interaction between a bdk wallet and miniscript policy.

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    // We start with a generic miniscript policy string
    let policy_str = "or(10@thresh(4,pk(029ffbe722b147f3035c87cb1c60b9a5947dd49c774cc31e94773478711a929ac0),pk(025f05815e3a1a8a83bfbb03ce016c9a2ee31066b98f567f6227df1d76ec4bd143),pk(025625f41e4a065efc06d5019cbbd56fe8c07595af1231e7cbc03fafb87ebb71ec),pk(02a27c8b850a00f67da3499b60562673dcf5fdfb82b7e17652a7ac54416812aefd),pk(03e618ec5f384d6e19ca9ebdb8e2119e5bef978285076828ce054e55c4daf473e2)),1@and(older(4209713),thresh(2,pk(03deae92101c790b12653231439f27b8897264125ecb2f46f48278603102573165),pk(033841045a531e1adf9910a6ec279589a90b3b8a904ee64ffd692bd08a8996c1aa),pk(02aebf2d10b040eb936a6f02f44ee82f8b34f5c1ccb20ff3949c2b28206b7c1068))))";
    info!("Compiling policy: \n{}", policy_str);

    // Parse the string as a [`Concrete`] type miniscript policy.
    let policy = Concrete::<String>::from_str(policy_str)?;

    // Create a `wsh` type descriptor from the policy.
    // `policy.compile()` returns the resulting miniscript from the policy.
    let descriptor = Descriptor::new_wsh(policy.compile()?)?;

    info!("Compiled into following Descriptor: \n{}", descriptor);

    let database = MemoryDatabase::new();

    // Create a new wallet from this descriptor
    let wallet = Wallet::new(&format!("{}", descriptor), None, Network::Regtest, database)?;

    info!(
        "First derived address from the descriptor: \n{}",
        wallet.get_address(New)?
    );

    // BDK also has it's own `Policy` structure to represent the spending condition in a more
    // human readable json format.
    let spending_policy = wallet.policies(KeychainKind::External)?;
    info!(
        "The BDK spending policy: \n{}",
        serde_json::to_string_pretty(&spending_policy)?
    );

    Ok(())
}
