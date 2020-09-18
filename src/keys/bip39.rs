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

//! BIP-0039

// TODO: maybe write our own implementation of bip39? Seems stupid to have an extra dependency for
// something that should be fairly simple to re-implement.

use bitcoin::util::bip32;
use bitcoin::Network;

use bip39::{Mnemonic, Seed};

use super::{DescriptorKey, ToDescriptorKey};
use crate::Error;

pub type MnemonicWithPassphrase = (Mnemonic, Option<String>);

impl ToDescriptorKey for (Seed, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        let xprv = bip32::ExtendedPrivKey::new_master(Network::Bitcoin, &self.0.as_bytes())?;
        (xprv, self.1).to_descriptor_key()
    }
}

impl ToDescriptorKey for (MnemonicWithPassphrase, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        let (mnemonic, passphrase) = self.0;
        let seed = Seed::new(&mnemonic, passphrase.as_deref().unwrap_or(""));
        (seed, self.1).to_descriptor_key()
    }
}

impl ToDescriptorKey for (Mnemonic, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        ((self.0, None), self.1).to_descriptor_key()
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::util::bip32;

    use bip39::{Language, Mnemonic};

    #[test]
    fn test_keys_bip39_mnemonic() {
        let mnemonic =
            "aim bunker wash balance finish force paper analyst cabin spoon stable organ";
        let mnemonic = Mnemonic::from_phrase(mnemonic, Language::English).unwrap();
        let path = bip32::DerivationPath::from_str("m/44'/0'/0'/0").unwrap();

        let key = (mnemonic, path);
        let (desc, keys) = crate::descriptor!(wpkh(key)).unwrap();
        assert_eq!(desc.to_string(), "wpkh([be83839f/44'/0'/0']xpub6DCQ1YcqvZtSwGWMrwHELPehjWV3f2MGZ69yBADTxFEUAoLwb5Mp5GniQK6tTp3AgbngVz9zEFbBJUPVnkG7LFYt8QMTfbrNqs6FNEwAPKA/0/*)");
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn test_keys_bip39_mnemonic_passphrase() {
        let mnemonic =
            "aim bunker wash balance finish force paper analyst cabin spoon stable organ";
        let mnemonic = Mnemonic::from_phrase(mnemonic, Language::English).unwrap();
        let path = bip32::DerivationPath::from_str("m/44'/0'/0'/0").unwrap();

        let key = ((mnemonic, Some("passphrase".into())), path);
        let (desc, keys) = crate::descriptor!(wpkh(key)).unwrap();
        assert_eq!(desc.to_string(), "wpkh([8f6cb80c/44'/0'/0']xpub6DWYS8bbihFevy29M4cbw4ZR3P5E12jB8R88gBDWCTCNpYiDHhYWNywrCF9VZQYagzPmsZpxXpytzSoxynyeFr4ZyzheVjnpLKuse4fiwZw/0/*)");
        assert_eq!(keys.len(), 1);
    }
}
