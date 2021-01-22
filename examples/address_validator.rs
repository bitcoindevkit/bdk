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
