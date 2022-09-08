// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bdk::bitcoin::secp256k1::Secp256k1;
use bdk::bitcoin::util::bip32::DerivationPath;
use bdk::bitcoin::Network;
use bdk::descriptor;
use bdk::descriptor::IntoWalletDescriptor;
use bdk::keys::bip39::{Language, Mnemonic, WordCount};
use bdk::keys::{GeneratableKey, GeneratedKey};
use bdk::miniscript::Tap;
use bdk::Error as BDK_Error;
use std::error::Error;
use std::str::FromStr;

/// This example demonstrates how to generate a mnemonic phrase
/// using BDK and use that to generate a descriptor string.
fn main() -> Result<(), Box<dyn Error>> {
    let secp = Secp256k1::new();

    // In this example we are generating a 12 words mnemonic phrase
    // but it is also possible generate 15, 18, 21 and 24 words
    // using their respective `WordCount` variant.
    let mnemonic: GeneratedKey<_, Tap> =
        Mnemonic::generate((WordCount::Words12, Language::English))
            .map_err(|_| BDK_Error::Generic("Mnemonic generation error".to_string()))?;

    println!("Mnemonic phrase: {}", *mnemonic);
    let mnemonic_with_passphrase = (mnemonic, None);

    // define external and internal derivation key path
    let external_path = DerivationPath::from_str("m/86h/0h/0h/0").unwrap();
    let internal_path = DerivationPath::from_str("m/86h/0h/0h/1").unwrap();

    // generate external and internal descriptor from mnemonic
    let (external_descriptor, ext_keymap) =
        descriptor!(tr((mnemonic_with_passphrase.clone(), external_path)))?
            .into_wallet_descriptor(&secp, Network::Testnet)?;
    let (internal_descriptor, int_keymap) =
        descriptor!(tr((mnemonic_with_passphrase, internal_path)))?
            .into_wallet_descriptor(&secp, Network::Testnet)?;

    println!("tpub external descriptor: {}", external_descriptor);
    println!("tpub internal descriptor: {}", internal_descriptor);
    println!(
        "tprv external descriptor: {}",
        external_descriptor.to_string_with_secret(&ext_keymap)
    );
    println!(
        "tprv internal descriptor: {}",
        internal_descriptor.to_string_with_secret(&int_keymap)
    );

    Ok(())
}
