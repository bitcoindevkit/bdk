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

use miniscript::ScriptContext;

use bip39::{Language, Mnemonic, MnemonicType, Seed};

use super::{any_network, DerivableKey, DescriptorKey, GeneratableKey, GeneratedKey, KeyError};

pub type MnemonicWithPassphrase = (Mnemonic, Option<String>);

impl<Ctx: ScriptContext> DerivableKey<Ctx> for Seed {
    fn add_metadata(
        self,
        source: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let xprv = bip32::ExtendedPrivKey::new_master(Network::Bitcoin, &self.as_bytes())?;
        let descriptor_key = xprv.add_metadata(source, derivation_path)?;

        // here we must choose one network to build the xpub, but since the bip39 standard doesn't
        // encode the network, the xpub we create is actually valid everywhere. so we override the
        // valid networks with `any_network()`.
        Ok(descriptor_key.override_valid_networks(any_network()))
    }
}

impl<Ctx: ScriptContext> DerivableKey<Ctx> for MnemonicWithPassphrase {
    fn add_metadata(
        self,
        source: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let (mnemonic, passphrase) = self;
        let seed = Seed::new(&mnemonic, passphrase.as_deref().unwrap_or(""));
        seed.add_metadata(source, derivation_path)
    }
}

impl<Ctx: ScriptContext> DerivableKey<Ctx> for Mnemonic {
    fn add_metadata(
        self,
        source: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        (self, None).add_metadata(source, derivation_path)
    }
}

impl<Ctx: ScriptContext> GeneratableKey<Ctx> for Mnemonic {
    type Entropy = [u8; 32];

    type Options = (MnemonicType, Language);
    type Error = Option<bip39::ErrorKind>;

    fn generate_with_entropy(
        (mnemonic_type, language): Self::Options,
        entropy: Self::Entropy,
    ) -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        let entropy = &entropy.as_ref()[..(mnemonic_type.entropy_bits() / 8)];
        let mnemonic = Mnemonic::from_entropy(entropy, language).map_err(|e| e.downcast().ok())?;

        Ok(GeneratedKey::new(mnemonic, any_network()))
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::util::bip32;

    use bip39::{Language, Mnemonic, MnemonicType};

    use crate::keys::{any_network, GeneratableKey, GeneratedKey};

    #[test]
    fn test_keys_bip39_mnemonic() {
        let mnemonic =
            "aim bunker wash balance finish force paper analyst cabin spoon stable organ";
        let mnemonic = Mnemonic::from_phrase(mnemonic, Language::English).unwrap();
        let path = bip32::DerivationPath::from_str("m/44'/0'/0'/0").unwrap();

        let key = (mnemonic, path);
        let (desc, keys, networks) = crate::descriptor!(wpkh(key)).unwrap();
        assert_eq!(desc.to_string(), "wpkh([be83839f/44'/0'/0']xpub6DCQ1YcqvZtSwGWMrwHELPehjWV3f2MGZ69yBADTxFEUAoLwb5Mp5GniQK6tTp3AgbngVz9zEFbBJUPVnkG7LFYt8QMTfbrNqs6FNEwAPKA/0/*)");
        assert_eq!(keys.len(), 1);
        assert_eq!(networks.len(), 3);
    }

    #[test]
    fn test_keys_bip39_mnemonic_passphrase() {
        let mnemonic =
            "aim bunker wash balance finish force paper analyst cabin spoon stable organ";
        let mnemonic = Mnemonic::from_phrase(mnemonic, Language::English).unwrap();
        let path = bip32::DerivationPath::from_str("m/44'/0'/0'/0").unwrap();

        let key = ((mnemonic, Some("passphrase".into())), path);
        let (desc, keys, networks) = crate::descriptor!(wpkh(key)).unwrap();
        assert_eq!(desc.to_string(), "wpkh([8f6cb80c/44'/0'/0']xpub6DWYS8bbihFevy29M4cbw4ZR3P5E12jB8R88gBDWCTCNpYiDHhYWNywrCF9VZQYagzPmsZpxXpytzSoxynyeFr4ZyzheVjnpLKuse4fiwZw/0/*)");
        assert_eq!(keys.len(), 1);
        assert_eq!(networks.len(), 3);
    }

    #[test]
    fn test_keys_generate_bip39() {
        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate_with_entropy(
                (MnemonicType::Words12, Language::English),
                crate::keys::test::get_test_entropy(),
            )
            .unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());
        assert_eq!(
            generated_mnemonic.to_string(),
            "primary fetch primary fetch primary fetch primary fetch primary fetch primary fever"
        );

        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate_with_entropy(
                (MnemonicType::Words24, Language::English),
                crate::keys::test::get_test_entropy(),
            )
            .unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());
        assert_eq!(generated_mnemonic.to_string(), "primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary foster");
    }

    #[test]
    fn test_keys_generate_bip39_random() {
        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate((MnemonicType::Words12, Language::English)).unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());

        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate((MnemonicType::Words24, Language::English)).unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());
    }
}
