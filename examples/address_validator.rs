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

use std::sync::Arc;

use bdk::bitcoin;
use bdk::database::MemoryDatabase;
use bdk::descriptor::HDKeyPaths;
use bdk::wallet::address_validator::{AddressValidator, AddressValidatorError};
use bdk::KeychainKind;
use bdk::Wallet;

use bitcoin::hashes::hex::FromHex;
use bitcoin::util::bip32::Fingerprint;
use bitcoin::{Network, Script};

#[derive(Debug)]
struct DummyValidator;
impl AddressValidator for DummyValidator {
    fn validate(
        &self,
        keychain: KeychainKind,
        hd_keypaths: &HDKeyPaths,
        script: &Script,
    ) -> Result<(), AddressValidatorError> {
        let (_, path) = hd_keypaths
            .values()
            .find(|(fing, _)| fing == &Fingerprint::from_hex("bc123c3e").unwrap())
            .ok_or(AddressValidatorError::InvalidScript)?;

        println!(
            "Validating `{:?}` {} address, script: {}",
            keychain, path, script
        );

        Ok(())
    }
}

fn main() -> Result<(), bdk::Error> {
    let descriptor = "sh(and_v(v:pk(tpubDDpWvmUrPZrhSPmUzCMBHffvC3HyMAPnWDSAQNBTnj1iZeJa7BZQEttFiP4DS4GCcXQHezdXhn86Hj6LHX5EDstXPWrMaSneRWM8yUf6NFd/*),after(630000)))";
    let mut wallet =
        Wallet::new_offline(descriptor, None, Network::Regtest, MemoryDatabase::new())?;

    wallet.add_address_validator(Arc::new(DummyValidator));

    wallet.get_new_address()?;
    wallet.get_new_address()?;
    wallet.get_new_address()?;

    Ok(())
}
