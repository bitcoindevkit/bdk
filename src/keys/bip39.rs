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

//! BIP-0039

// TODO: maybe write our own implementation of bip39? Seems stupid to have an extra dependency for
// something that should be fairly simple to re-implement.

use bitcoin::util::bip32;
use bitcoin::Network;

use miniscript::ScriptContext;

pub use bip39::{Error, Language, Mnemonic};

type Seed = [u8; 64];

/// Type describing entropy length (aka word count) in the mnemonic
pub enum WordCount {
    /// 12 words mnemonic (128 bits entropy)
    Words12 = 128,
    /// 15 words mnemonic (160 bits entropy)
    Words15 = 160,
    /// 18 words mnemonic (192 bits entropy)
    Words18 = 192,
    /// 21 words mnemonic (224 bits entropy)
    Words21 = 224,
    /// 24 words mnemonic (256 bits entropy)
    Words24 = 256,
}

use super::{
    any_network, DerivableKey, DescriptorKey, ExtendedKey, GeneratableKey, GeneratedKey, KeyError,
};

fn set_valid_on_any_network<Ctx: ScriptContext>(
    descriptor_key: DescriptorKey<Ctx>,
) -> DescriptorKey<Ctx> {
    // We have to pick one network to build the xprv, but since the bip39 standard doesn't
    // encode the network, the xprv we create is actually valid everywhere. So we override the
    // valid networks with `any_network()`.
    descriptor_key.override_valid_networks(any_network())
}

/// Type for a BIP39 mnemonic with an optional passphrase
pub type MnemonicWithPassphrase = (Mnemonic, Option<String>);

#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
impl<Ctx: ScriptContext> DerivableKey<Ctx> for Seed {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        Ok(bip32::ExtendedPrivKey::new_master(Network::Bitcoin, &self[..])?.into())
    }

    fn into_descriptor_key(
        self,
        source: Option<bip32::KeySource>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let descriptor_key = self
            .into_extended_key()?
            .into_descriptor_key(source, derivation_path)?;

        Ok(set_valid_on_any_network(descriptor_key))
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
impl<Ctx: ScriptContext> DerivableKey<Ctx> for MnemonicWithPassphrase {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        let (mnemonic, passphrase) = self;
        let seed: Seed = mnemonic.to_seed(passphrase.as_deref().unwrap_or(""));

        seed.into_extended_key()
    }

    fn into_descriptor_key(
        self,
        source: Option<bip32::KeySource>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let descriptor_key = self
            .into_extended_key()?
            .into_descriptor_key(source, derivation_path)?;

        Ok(set_valid_on_any_network(descriptor_key))
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
impl<Ctx: ScriptContext> DerivableKey<Ctx> for (GeneratedKey<Mnemonic, Ctx>, Option<String>) {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        let (mnemonic, passphrase) = self;
        (mnemonic.into_key(), passphrase).into_extended_key()
    }

    fn into_descriptor_key(
        self,
        source: Option<bip32::KeySource>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let (mnemonic, passphrase) = self;
        (mnemonic.into_key(), passphrase).into_descriptor_key(source, derivation_path)
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
impl<Ctx: ScriptContext> DerivableKey<Ctx> for Mnemonic {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        (self, None).into_extended_key()
    }

    fn into_descriptor_key(
        self,
        source: Option<bip32::KeySource>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let descriptor_key = self
            .into_extended_key()?
            .into_descriptor_key(source, derivation_path)?;

        Ok(set_valid_on_any_network(descriptor_key))
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
impl<Ctx: ScriptContext> GeneratableKey<Ctx> for Mnemonic {
    type Entropy = [u8; 32];

    type Options = (WordCount, Language);
    type Error = Option<bip39::Error>;

    fn generate_with_entropy(
        (word_count, language): Self::Options,
        entropy: Self::Entropy,
    ) -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        let entropy = &entropy.as_ref()[..(word_count as usize / 8)];
        let mnemonic = Mnemonic::from_entropy_in(language, entropy)?;

        Ok(GeneratedKey::new(mnemonic, any_network()))
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::util::bip32;

    use bip39::{Language, Mnemonic};

    use crate::keys::{any_network, GeneratableKey, GeneratedKey};

    use super::WordCount;

    #[test]
    fn test_keys_bip39_mnemonic() {
        let mnemonic =
            "aim bunker wash balance finish force paper analyst cabin spoon stable organ";
        let mnemonic = Mnemonic::parse_in(Language::English, mnemonic).unwrap();
        let path = bip32::DerivationPath::from_str("m/44'/0'/0'/0").unwrap();

        let key = (mnemonic, path);
        let (desc, keys, networks) = crate::descriptor!(wpkh(key)).unwrap();
        assert_eq!(desc.to_string(), "wpkh([be83839f/44'/0'/0']xpub6DCQ1YcqvZtSwGWMrwHELPehjWV3f2MGZ69yBADTxFEUAoLwb5Mp5GniQK6tTp3AgbngVz9zEFbBJUPVnkG7LFYt8QMTfbrNqs6FNEwAPKA/0/*)#0r8v4nkv");
        assert_eq!(keys.len(), 1);
        assert_eq!(networks.len(), 4);
    }

    #[test]
    fn test_keys_bip39_mnemonic_passphrase() {
        let mnemonic =
            "aim bunker wash balance finish force paper analyst cabin spoon stable organ";
        let mnemonic = Mnemonic::parse_in(Language::English, mnemonic).unwrap();
        let path = bip32::DerivationPath::from_str("m/44'/0'/0'/0").unwrap();

        let key = ((mnemonic, Some("passphrase".into())), path);
        let (desc, keys, networks) = crate::descriptor!(wpkh(key)).unwrap();
        assert_eq!(desc.to_string(), "wpkh([8f6cb80c/44'/0'/0']xpub6DWYS8bbihFevy29M4cbw4ZR3P5E12jB8R88gBDWCTCNpYiDHhYWNywrCF9VZQYagzPmsZpxXpytzSoxynyeFr4ZyzheVjnpLKuse4fiwZw/0/*)#h0j0tg5m");
        assert_eq!(keys.len(), 1);
        assert_eq!(networks.len(), 4);
    }

    #[test]
    fn test_keys_generate_bip39() {
        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate_with_entropy(
                (WordCount::Words12, Language::English),
                crate::keys::test::TEST_ENTROPY,
            )
            .unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());
        assert_eq!(
            generated_mnemonic.to_string(),
            "primary fetch primary fetch primary fetch primary fetch primary fetch primary fever"
        );

        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate_with_entropy(
                (WordCount::Words24, Language::English),
                crate::keys::test::TEST_ENTROPY,
            )
            .unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());
        assert_eq!(generated_mnemonic.to_string(), "primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary fetch primary foster");
    }

    #[test]
    fn test_keys_generate_bip39_random() {
        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate((WordCount::Words12, Language::English)).unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());

        let generated_mnemonic: GeneratedKey<_, miniscript::Segwitv0> =
            Mnemonic::generate((WordCount::Words24, Language::English)).unwrap();
        assert_eq!(generated_mnemonic.valid_networks, any_network());
    }
}
