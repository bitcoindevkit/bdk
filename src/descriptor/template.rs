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

//! Descriptor templates
//!
//! This module contains the definition of various common script templates that are ready to be
//! used. See the documentation of each template for an example.

use bitcoin::util::bip32;
use bitcoin::Network;

use miniscript::{Legacy, Segwitv0};

use super::{ExtendedDescriptor, KeyMap, ToWalletDescriptor};
use crate::keys::{DerivableKey, KeyError, ToDescriptorKey, ValidNetworks};
use crate::{descriptor, ScriptType};

/// Type alias for the return type of [`DescriptorTemplate`], [`descriptor!`](crate::descriptor!) and others
pub type DescriptorTemplateOut = (ExtendedDescriptor, KeyMap, ValidNetworks);

/// Trait for descriptor templates that can be built into a full descriptor
///
/// Since [`ToWalletDescriptor`] is implemented for any [`DescriptorTemplate`], they can also be
/// passed directly to the [`Wallet`](crate::Wallet) constructor.
///
/// ## Example
///
/// ```
/// use bdk::keys::{ToDescriptorKey, KeyError};
/// use bdk::template::{DescriptorTemplate, DescriptorTemplateOut};
/// use bdk::miniscript::Legacy;
///
/// struct MyP2PKH<K: ToDescriptorKey<Legacy>>(K);
///
/// impl<K: ToDescriptorKey<Legacy>> DescriptorTemplate for MyP2PKH<K> {
///     fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
///         Ok(bdk::descriptor!(pkh ( self.0 ) )?)
///     }
/// }
/// ```
pub trait DescriptorTemplate {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError>;
}

/// Turns a [`DescriptorTemplate`] into a valid wallet descriptor by calling its
/// [`build`](DescriptorTemplate::build) method
impl<T: DescriptorTemplate> ToWalletDescriptor for T {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError> {
        Ok(self.build()?.to_wallet_descriptor(network)?)
    }
}

/// P2PKH template. Expands to a descriptor `pkh(key)`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::P2PKH;
///
/// let key = bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(P2PKH(key), None, Network::Testnet, MemoryDatabase::default())?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "mwJ8hxFYW19JLuc65RCTaP4v1rzVU8cVMT");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct P2PKH<K: ToDescriptorKey<Legacy>>(pub K);

impl<K: ToDescriptorKey<Legacy>> DescriptorTemplate for P2PKH<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(descriptor!(pkh(self.0))?)
    }
}

/// P2WPKH-P2SH template. Expands to a descriptor `sh(wpkh(key))`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::P2WPKH_P2SH;
///
/// let key = bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(P2WPKH_P2SH(key), None, Network::Testnet, MemoryDatabase::default())?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "2NB4ox5VDRw1ecUv6SnT3VQHPXveYztRqk5");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
#[allow(non_camel_case_types)]
pub struct P2WPKH_P2SH<K: ToDescriptorKey<Segwitv0>>(pub K);

impl<K: ToDescriptorKey<Segwitv0>> DescriptorTemplate for P2WPKH_P2SH<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(descriptor!(sh(wpkh(self.0)))?)
    }
}

/// P2WPKH template. Expands to a descriptor `wpkh(key)`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::P2WPKH;
///
/// let key = bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(P2WPKH(key), None, Network::Testnet, MemoryDatabase::default())?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "tb1q4525hmgw265tl3drrl8jjta7ayffu6jf68ltjd");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct P2WPKH<K: ToDescriptorKey<Segwitv0>>(pub K);

impl<K: ToDescriptorKey<Segwitv0>> DescriptorTemplate for P2WPKH<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(descriptor!(wpkh(self.0))?)
    }
}

/// BIP44 template. Expands to `pkh(key/44'/0'/0'/{0,1}/*)`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`BIP44Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet, ScriptType};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::BIP44;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(
///     BIP44(key.clone(), ScriptType::External),
///     Some(BIP44(key, ScriptType::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "miNG7dJTzJqNbFS19svRdTCisC65dsubtR");
/// assert_eq!(wallet.public_descriptor(ScriptType::External)?.unwrap().to_string(), "pkh([c55b303f/44'/0'/0']tpubDDDzQ31JkZB7VxUr9bjvBivDdqoFLrDPyLWtLapArAi51ftfmCb2DPxwLQzX65iNcXz1DGaVvyvo6JQ6rTU73r2gqdEo8uov9QKRb7nKCSU/0/*)");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct BIP44<K: DerivableKey<Legacy>>(pub K, pub ScriptType);

impl<K: DerivableKey<Legacy>> DescriptorTemplate for BIP44<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(P2PKH(legacy::make_bipxx_private(44, self.0, self.1)?).build()?)
    }
}

/// BIP44 public template. Expands to `pkh(key/{0,1}/*)`
///
/// This assumes that the key used has already been derived with `m/44'/0'/0'`.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`BIP44`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet, ScriptType};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::BIP44Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDDDzQ31JkZB7VxUr9bjvBivDdqoFLrDPyLWtLapArAi51ftfmCb2DPxwLQzX65iNcXz1DGaVvyvo6JQ6rTU73r2gqdEo8uov9QKRb7nKCSU")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(
///     BIP44Public(key.clone(), fingerprint, ScriptType::External),
///     Some(BIP44Public(key, fingerprint, ScriptType::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "miNG7dJTzJqNbFS19svRdTCisC65dsubtR");
/// assert_eq!(wallet.public_descriptor(ScriptType::External)?.unwrap().to_string(), "pkh([c55b303f/44'/0'/0']tpubDDDzQ31JkZB7VxUr9bjvBivDdqoFLrDPyLWtLapArAi51ftfmCb2DPxwLQzX65iNcXz1DGaVvyvo6JQ6rTU73r2gqdEo8uov9QKRb7nKCSU/0/*)");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct BIP44Public<K: DerivableKey<Legacy>>(pub K, pub bip32::Fingerprint, pub ScriptType);

impl<K: DerivableKey<Legacy>> DescriptorTemplate for BIP44Public<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(P2PKH(legacy::make_bipxx_public(44, self.0, self.1, self.2)?).build()?)
    }
}

/// BIP49 template. Expands to `sh(wpkh(key/49'/0'/0'/{0,1}/*))`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`BIP49Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet, ScriptType};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::BIP49;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(
///     BIP49(key.clone(), ScriptType::External),
///     Some(BIP49(key, ScriptType::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "2N3K4xbVAHoiTQSwxkZjWDfKoNC27pLkYnt");
/// assert_eq!(wallet.public_descriptor(ScriptType::External)?.unwrap().to_string(), "sh(wpkh([c55b303f/49\'/0\'/0\']tpubDC49r947KGK52X5rBWS4BLs5m9SRY3pYHnvRrm7HcybZ3BfdEsGFyzCMzayi1u58eT82ZeyFZwH7DD6Q83E3fM9CpfMtmnTygnLfP59jL9L/0/*))");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct BIP49<K: DerivableKey<Segwitv0>>(pub K, pub ScriptType);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for BIP49<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(P2WPKH_P2SH(segwit_v0::make_bipxx_private(49, self.0, self.1)?).build()?)
    }
}

/// BIP49 public template. Expands to `sh(wpkh(key/{0,1}/*))`
///
/// This assumes that the key used has already been derived with `m/49'/0'/0'`.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`BIP49`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet, ScriptType};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::BIP49Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC49r947KGK52X5rBWS4BLs5m9SRY3pYHnvRrm7HcybZ3BfdEsGFyzCMzayi1u58eT82ZeyFZwH7DD6Q83E3fM9CpfMtmnTygnLfP59jL9L")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(
///     BIP49Public(key.clone(), fingerprint, ScriptType::External),
///     Some(BIP49Public(key, fingerprint, ScriptType::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "2N3K4xbVAHoiTQSwxkZjWDfKoNC27pLkYnt");
/// assert_eq!(wallet.public_descriptor(ScriptType::External)?.unwrap().to_string(), "sh(wpkh([c55b303f/49\'/0\'/0\']tpubDC49r947KGK52X5rBWS4BLs5m9SRY3pYHnvRrm7HcybZ3BfdEsGFyzCMzayi1u58eT82ZeyFZwH7DD6Q83E3fM9CpfMtmnTygnLfP59jL9L/0/*))");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct BIP49Public<K: DerivableKey<Segwitv0>>(pub K, pub bip32::Fingerprint, pub ScriptType);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for BIP49Public<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(P2WPKH_P2SH(segwit_v0::make_bipxx_public(49, self.0, self.1, self.2)?).build()?)
    }
}

/// BIP84 template. Expands to `wpkh(key/84'/0'/0'/{0,1}/*)`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`BIP84Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet, ScriptType};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::BIP84;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(
///     BIP84(key.clone(), ScriptType::External),
///     Some(BIP84(key, ScriptType::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "tb1qedg9fdlf8cnnqfd5mks6uz5w4kgpk2pr6y4qc7");
/// assert_eq!(wallet.public_descriptor(ScriptType::External)?.unwrap().to_string(), "wpkh([c55b303f/84\'/0\'/0\']tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q/0/*)");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct BIP84<K: DerivableKey<Segwitv0>>(pub K, pub ScriptType);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for BIP84<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(P2WPKH(segwit_v0::make_bipxx_private(84, self.0, self.1)?).build()?)
    }
}

/// BIP84 public template. Expands to `wpkh(key/{0,1}/*)`
///
/// This assumes that the key used has already been derived with `m/84'/0'/0'`.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`BIP84`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet, OfflineWallet, ScriptType};
/// # use bdk::database::MemoryDatabase;
/// use bdk::template::BIP84Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let wallet: OfflineWallet<_> = Wallet::new_offline(
///     BIP84Public(key.clone(), fingerprint, ScriptType::External),
///     Some(BIP84Public(key, fingerprint, ScriptType::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_new_address()?.to_string(), "tb1qedg9fdlf8cnnqfd5mks6uz5w4kgpk2pr6y4qc7");
/// assert_eq!(wallet.public_descriptor(ScriptType::External)?.unwrap().to_string(), "wpkh([c55b303f/84\'/0\'/0\']tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q/0/*)");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct BIP84Public<K: DerivableKey<Segwitv0>>(pub K, pub bip32::Fingerprint, pub ScriptType);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for BIP84Public<K> {
    fn build(self) -> Result<DescriptorTemplateOut, KeyError> {
        Ok(P2WPKH(segwit_v0::make_bipxx_public(84, self.0, self.1, self.2)?).build()?)
    }
}

macro_rules! expand_make_bipxx {
    ( $mod_name:ident, $ctx:ty ) => {
        mod $mod_name {
            use super::*;

            pub(super) fn make_bipxx_private<K: DerivableKey<$ctx>>(
                bip: u32,
                key: K,
                script_type: ScriptType,
            ) -> Result<impl ToDescriptorKey<$ctx>, KeyError> {
                let mut derivation_path = Vec::with_capacity(4);
                derivation_path.push(bip32::ChildNumber::from_hardened_idx(bip)?);
                derivation_path.push(bip32::ChildNumber::from_hardened_idx(0)?);
                derivation_path.push(bip32::ChildNumber::from_hardened_idx(0)?);

                match script_type {
                    ScriptType::External => {
                        derivation_path.push(bip32::ChildNumber::from_normal_idx(0)?)
                    }
                    ScriptType::Internal => {
                        derivation_path.push(bip32::ChildNumber::from_normal_idx(1)?)
                    }
                };

                let derivation_path: bip32::DerivationPath = derivation_path.into();

                Ok((key, derivation_path))
            }
            pub(super) fn make_bipxx_public<K: DerivableKey<$ctx>>(
                bip: u32,
                key: K,
                parent_fingerprint: bip32::Fingerprint,
                script_type: ScriptType,
            ) -> Result<impl ToDescriptorKey<$ctx>, KeyError> {
                let derivation_path: bip32::DerivationPath = match script_type {
                    ScriptType::External => vec![bip32::ChildNumber::from_normal_idx(0)?].into(),
                    ScriptType::Internal => vec![bip32::ChildNumber::from_normal_idx(1)?].into(),
                };

                let mut source_path = Vec::with_capacity(3);
                source_path.push(bip32::ChildNumber::from_hardened_idx(bip)?);
                source_path.push(bip32::ChildNumber::from_hardened_idx(0)?);
                source_path.push(bip32::ChildNumber::from_hardened_idx(0)?);
                let source_path: bip32::DerivationPath = source_path.into();

                Ok((key, (parent_fingerprint, source_path), derivation_path))
            }
        }
    };
}

expand_make_bipxx!(legacy, Legacy);
expand_make_bipxx!(segwit_v0, Segwitv0);

#[cfg(test)]
mod test {
    // test existing descriptor templates, make sure they are expanded to the right descriptors

    use super::*;
    use crate::descriptor::DescriptorMeta;
    use crate::keys::{KeyError, ValidNetworks};
    use bitcoin::hashes::core::str::FromStr;
    use bitcoin::network::constants::Network::Regtest;
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::util::bip32::ChildNumber;
    use miniscript::descriptor::{DescriptorPublicKey, DescriptorPublicKeyCtx, KeyMap};
    use miniscript::Descriptor;

    // verify template descriptor generates expected address(es)
    fn check(
        desc: Result<(Descriptor<DescriptorPublicKey>, KeyMap, ValidNetworks), KeyError>,
        is_witness: bool,
        is_fixed: bool,
        expected: &[&str],
    ) {
        let secp = Secp256k1::new();
        let deriv_ctx =
            DescriptorPublicKeyCtx::new(&secp, ChildNumber::from_normal_idx(0).unwrap());

        let (desc, _key_map, _networks) = desc.unwrap();
        assert_eq!(desc.is_witness(), is_witness);
        assert_eq!(desc.is_fixed(), is_fixed);
        for i in 0..expected.len() {
            let index = i as u32;
            let child_desc = if desc.is_fixed() {
                desc.clone()
            } else {
                desc.derive(ChildNumber::from_normal_idx(index).unwrap())
            };
            let address = child_desc.address(Regtest, deriv_ctx).unwrap();
            assert_eq!(address.to_string(), *expected.get(i).unwrap());
        }
    }

    // P2PKH
    #[test]
    fn test_p2ph_template() {
        let prvkey =
            bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")
                .unwrap();
        check(
            P2PKH(prvkey).build(),
            false,
            true,
            &["mwJ8hxFYW19JLuc65RCTaP4v1rzVU8cVMT"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2PKH(pubkey).build(),
            false,
            true,
            &["muZpTpBYhxmRFuCjLc7C6BBDF32C8XVJUi"],
        );
    }

    // P2WPKH-P2SH `sh(wpkh(key))`
    #[test]
    fn test_p2wphp2sh_template() {
        let prvkey =
            bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")
                .unwrap();
        check(
            P2WPKH_P2SH(prvkey).build(),
            true,
            true,
            &["2NB4ox5VDRw1ecUv6SnT3VQHPXveYztRqk5"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2WPKH_P2SH(pubkey).build(),
            true,
            true,
            &["2N5LiC3CqzxDamRTPG1kiNv1FpNJQ7x28sb"],
        );
    }

    // P2WPKH `wpkh(key)`
    #[test]
    fn test_p2wph_template() {
        let prvkey =
            bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")
                .unwrap();
        check(
            P2WPKH(prvkey).build(),
            true,
            true,
            &["bcrt1q4525hmgw265tl3drrl8jjta7ayffu6jfcwxx9y"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2WPKH(pubkey).build(),
            true,
            true,
            &["bcrt1qngw83fg8dz0k749cg7k3emc7v98wy0c7azaa6h"],
        );
    }

    // BIP44 `pkh(key/44'/0'/0'/{0,1}/*)`
    #[test]
    fn test_bip44_template() {
        let prvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        check(
            BIP44(prvkey, ScriptType::External).build(),
            false,
            false,
            &[
                "n453VtnjDHPyDt2fDstKSu7A3YCJoHZ5g5",
                "mvfrrumXgTtwFPWDNUecBBgzuMXhYM7KRP",
                "mzYvhRAuQqbdSKMVVzXNYyqihgNdRadAUQ",
            ],
        );
        check(
            BIP44(prvkey, ScriptType::Internal).build(),
            false,
            false,
            &[
                "muHF98X9KxEzdKrnFAX85KeHv96eXopaip",
                "n4hpyLJE5ub6B5Bymv4eqFxS5KjrewSmYR",
                "mgvkdv1ffmsXd2B1sRKQ5dByK3SzpG42rA",
            ],
        );
    }

    // BIP44 public `pkh(key/{0,1}/*)`
    #[test]
    fn test_bip44_public_template() {
        let pubkey = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDDDzQ31JkZB7VxUr9bjvBivDdqoFLrDPyLWtLapArAi51ftfmCb2DPxwLQzX65iNcXz1DGaVvyvo6JQ6rTU73r2gqdEo8uov9QKRb7nKCSU").unwrap();
        let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f").unwrap();
        check(
            BIP44Public(pubkey, fingerprint, ScriptType::External).build(),
            false,
            false,
            &[
                "miNG7dJTzJqNbFS19svRdTCisC65dsubtR",
                "n2UqaDbCjWSFJvpC84m3FjUk5UaeibCzYg",
                "muCPpS6Ue7nkzeJMWDViw7Lkwr92Yc4K8g",
            ],
        );
        check(
            BIP44Public(pubkey, fingerprint, ScriptType::Internal).build(),
            false,
            false,
            &[
                "moDr3vJ8wpt5nNxSK55MPq797nXJb2Ru9H",
                "ms7A1Yt4uTezT2XkefW12AvLoko8WfNJMG",
                "mhYiyat2rtEnV77cFfQsW32y1m2ceCGHPo",
            ],
        );
    }

    // BIP49 `sh(wpkh(key/49'/0'/0'/{0,1}/*))`
    #[test]
    fn test_bip49_template() {
        let prvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        check(
            BIP49(prvkey, ScriptType::External).build(),
            true,
            false,
            &[
                "2N9bCAJXGm168MjVwpkBdNt6ucka3PKVoUV",
                "2NDckYkqrYyDMtttEav5hB3Bfw9EGAW5HtS",
                "2NAFTVtksF9T4a97M7nyCjwUBD24QevZ5Z4",
            ],
        );
        check(
            BIP49(prvkey, ScriptType::Internal).build(),
            true,
            false,
            &[
                "2NB3pA8PnzJLGV8YEKNDFpbViZv3Bm1K6CG",
                "2NBiX2Wzxngb5rPiWpUiJQ2uLVB4HBjFD4p",
                "2NA8ek4CdQ6aMkveYF6AYuEYNrftB47QGTn",
            ],
        );
    }

    // BIP49 public `sh(wpkh(key/{0,1}/*))`
    #[test]
    fn test_bip49_public_template() {
        let pubkey = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC49r947KGK52X5rBWS4BLs5m9SRY3pYHnvRrm7HcybZ3BfdEsGFyzCMzayi1u58eT82ZeyFZwH7DD6Q83E3fM9CpfMtmnTygnLfP59jL9L").unwrap();
        let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f").unwrap();
        check(
            BIP49Public(pubkey, fingerprint, ScriptType::External).build(),
            true,
            false,
            &[
                "2N3K4xbVAHoiTQSwxkZjWDfKoNC27pLkYnt",
                "2NCTQfJ1sZa3wQ3pPseYRHbaNEpC3AquEfX",
                "2MveFxAuC8BYPzTybx7FxSzW8HSd8ATT4z7",
            ],
        );
        check(
            BIP49Public(pubkey, fingerprint, ScriptType::Internal).build(),
            true,
            false,
            &[
                "2NF2vttKibwyxigxtx95Zw8K7JhDbo5zPVJ",
                "2Mtmyd8taksxNVWCJ4wVvaiss7QPZGcAJuH",
                "2NBs3CTVYPr1HCzjB4YFsnWCPCtNg8uMEfp",
            ],
        );
    }

    // BIP84 `wpkh(key/84'/0'/0'/{0,1}/*)`
    #[test]
    fn test_bip84_template() {
        let prvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        check(
            BIP84(prvkey, ScriptType::External).build(),
            true,
            false,
            &[
                "bcrt1qkmvk2nadgplmd57ztld8nf8v2yxkzmdvwtjf8s",
                "bcrt1qx0v6zgfwe50m4kqc58cqzcyem7ay2sfl3gvqhp",
                "bcrt1q4h7fq9zhxst6e69p3n882nfj649l7w9g3zccfp",
            ],
        );
        check(
            BIP84(prvkey, ScriptType::Internal).build(),
            true,
            false,
            &[
                "bcrt1qtrwtz00wxl69e5xex7amy4xzlxkaefg3gfdkxa",
                "bcrt1qqqasfhxpkkf7zrxqnkr2sfhn74dgsrc3e3ky45",
                "bcrt1qpks7n0gq74hsgsz3phn5vuazjjq0f5eqhsgyce",
            ],
        );
    }

    // BIP84 public `wpkh(key/{0,1}/*)`
    #[test]
    fn test_bip84_public_template() {
        let pubkey = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q").unwrap();
        let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f").unwrap();
        check(
            BIP84Public(pubkey, fingerprint, ScriptType::External).build(),
            true,
            false,
            &[
                "bcrt1qedg9fdlf8cnnqfd5mks6uz5w4kgpk2prcdvd0h",
                "bcrt1q3lncdlwq3lgcaaeyruynjnlccr0ve0kakh6ana",
                "bcrt1qt9800y6xl3922jy3uyl0z33jh5wfpycyhcylr9",
            ],
        );
        check(
            BIP84Public(pubkey, fingerprint, ScriptType::Internal).build(),
            true,
            false,
            &[
                "bcrt1qm6wqukenh7guu792lj2njgw9n78cmwsy8xy3z2",
                "bcrt1q694twxtjn4nnrvnyvra769j0a23rllj5c6cgwp",
                "bcrt1qhlac3c5ranv5w5emlnqs7wxhkxt8maelylcarp",
            ],
        );
    }
}
