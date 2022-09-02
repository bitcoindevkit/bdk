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
extern crate env_logger;
extern crate log;
use std::error::Error;

use bdk::bitcoin::Network;
use bdk::descriptor::{policy::BuildSatisfaction, ExtractPolicy, IntoWalletDescriptor};
use bdk::wallet::signer::SignersContainer;

/// This example describes the use of the BDK's [`bdk::descriptor::policy`] module.
///
/// Policy is higher abstraction representation of the wallet descriptor spending condition.
/// This is useful to express complex miniscript spending conditions into more human readable form.
/// The resulting `Policy` structure  can be used to derive spending conditions the wallet is capable
/// to spend from.
///
/// This example demos a Policy output for a 2of2 multisig between between 2 parties, where the wallet holds
/// one of the Extend Private key.

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let secp = bitcoin::secp256k1::Secp256k1::new();

    // The descriptor used in the example
    // The form is "wsh(multi(2, <privkey>, <pubkey>))"
    let desc = "wsh(multi(2,tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/*,tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/*))";

    // Use the descriptor string to derive the full descriptor and a keymap.
    // The wallet descriptor can be used to create a new bdk::wallet.
    // While the `keymap` can be used to create a `SignerContainer`.
    //
    // The `SignerContainer` can sign for `PSBT`s.
    // a bdk::wallet internally uses these to handle transaction signing.
    // But they can be used as independent tools also.
    let (wallet_desc, keymap) = desc.into_wallet_descriptor(&secp, Network::Testnet)?;

    log::info!("Example Descriptor for policy analysis : {}", wallet_desc);

    // Create the signer with the keymap and descriptor.
    let signers_container = SignersContainer::build(keymap, &wallet_desc, &secp);

    // Extract the Policy from the given descriptor and signer.
    // Note that Policy is a wallet specific structure. It depends on the the descriptor, and
    // what the concerned wallet with a given signer can sign for.
    let policy = wallet_desc
        .extract_policy(&signers_container, BuildSatisfaction::None, &secp)?
        .expect("We expect a policy");

    log::info!("Derived Policy for the descriptor {:#?}", policy);

    Ok(())
}
