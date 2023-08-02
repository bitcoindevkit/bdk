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

//! Descriptor templates
//!
//! This module contains the definition of various common script templates that are ready to be
//! used. See the documentation of each template for an example.

use bitcoin::util::bip32;
use bitcoin::Network;

use miniscript::{Legacy, Segwitv0, Tap};

use super::{ExtendedDescriptor, IntoWalletDescriptor, KeyMap};
use crate::descriptor::DescriptorError;
use crate::keys::{DerivableKey, IntoDescriptorKey, ValidNetworks};
use crate::wallet::utils::SecpCtx;
use crate::{descriptor, KeychainKind};

/// Type alias for the return type of [`DescriptorTemplate`], [`descriptor!`](crate::descriptor!) and others
pub type DescriptorTemplateOut = (ExtendedDescriptor, KeyMap, ValidNetworks);

/// Trait for descriptor templates that can be built into a full descriptor
///
/// Since [`IntoWalletDescriptor`] is implemented for any [`DescriptorTemplate`], they can also be
/// passed directly to the [`Wallet`](crate::Wallet) constructor.
///
/// ## Example
///
/// ```
/// use bdk::descriptor::error::Error as DescriptorError;
/// use bdk::keys::{IntoDescriptorKey, KeyError};
/// use bdk::miniscript::Legacy;
/// use bdk::template::{DescriptorTemplate, DescriptorTemplateOut};
/// use bitcoin::Network;
///
/// struct MyP2PKH<K: IntoDescriptorKey<Legacy>>(K);
///
/// impl<K: IntoDescriptorKey<Legacy>> DescriptorTemplate for MyP2PKH<K> {
///     fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
///         Ok(bdk::descriptor!(pkh(self.0))?)
///     }
/// }
/// ```
pub trait DescriptorTemplate {
    /// Build the complete descriptor
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError>;
}

/// Turns a [`DescriptorTemplate`] into a valid wallet descriptor by calling its
/// [`build`](DescriptorTemplate::build) method
impl<T: DescriptorTemplate> IntoWalletDescriptor for T {
    fn into_wallet_descriptor(
        self,
        secp: &SecpCtx,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), DescriptorError> {
        self.build(network)?.into_wallet_descriptor(secp, network)
    }
}

/// P2PKH template. Expands to a descriptor `pkh(key)`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::P2Pkh;
///
/// let key =
///     bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let wallet = Wallet::new(
///     P2Pkh(key),
///     None,
///     Network::Testnet,
///     MemoryDatabase::default(),
/// )?;
///
/// assert_eq!(
///     wallet.get_address(New)?.to_string(),
///     "mwJ8hxFYW19JLuc65RCTaP4v1rzVU8cVMT"
/// );
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct P2Pkh<K: IntoDescriptorKey<Legacy>>(pub K);

impl<K: IntoDescriptorKey<Legacy>> DescriptorTemplate for P2Pkh<K> {
    fn build(self, _network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        descriptor!(pkh(self.0))
    }
}

/// P2WPKH-P2SH template. Expands to a descriptor `sh(wpkh(key))`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::P2Wpkh_P2Sh;
///
/// let key =
///     bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let wallet = Wallet::new(
///     P2Wpkh_P2Sh(key),
///     None,
///     Network::Testnet,
///     MemoryDatabase::default(),
/// )?;
///
/// assert_eq!(
///     wallet.get_address(New)?.to_string(),
///     "2NB4ox5VDRw1ecUv6SnT3VQHPXveYztRqk5"
/// );
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
#[allow(non_camel_case_types)]
pub struct P2Wpkh_P2Sh<K: IntoDescriptorKey<Segwitv0>>(pub K);

impl<K: IntoDescriptorKey<Segwitv0>> DescriptorTemplate for P2Wpkh_P2Sh<K> {
    fn build(self, _network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        descriptor!(sh(wpkh(self.0)))
    }
}

/// P2WPKH template. Expands to a descriptor `wpkh(key)`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::P2Wpkh;
///
/// let key =
///     bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let wallet = Wallet::new(
///     P2Wpkh(key),
///     None,
///     Network::Testnet,
///     MemoryDatabase::default(),
/// )?;
///
/// assert_eq!(
///     wallet.get_address(New)?.to_string(),
///     "tb1q4525hmgw265tl3drrl8jjta7ayffu6jf68ltjd"
/// );
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct P2Wpkh<K: IntoDescriptorKey<Segwitv0>>(pub K);

impl<K: IntoDescriptorKey<Segwitv0>> DescriptorTemplate for P2Wpkh<K> {
    fn build(self, _network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        descriptor!(wpkh(self.0))
    }
}

/// P2TR template. Expands to a descriptor `tr(key)`
///
/// ## Example
///
/// ```
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::Wallet;
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::P2TR;
///
/// let key =
///     bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")?;
/// let mut wallet = Wallet::new(P2TR(key), None, Network::Testnet, MemoryDatabase::default())?;
///
/// assert_eq!(
///     wallet.get_address(New)?.to_string(),
///     "tb1pvjf9t34fznr53u5tqhejz4nr69luzkhlvsdsdfq9pglutrpve2xq7hps46"
/// );
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct P2TR<K: IntoDescriptorKey<Tap>>(pub K);

impl<K: IntoDescriptorKey<Tap>> DescriptorTemplate for P2TR<K> {
    fn build(self, _network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        descriptor!(tr(self.0))
    }
}

/// BIP44 template. Expands to `pkh(key/44'/{0,1}'/0'/{0,1}/*)`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`Bip44Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip44;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let wallet = Wallet::new(
///     Bip44(key.clone(), KeychainKind::External),
///     Some(Bip44(key, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "mmogjc7HJEZkrLqyQYqJmxUqFaC7i4uf89");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "pkh([c55b303f/44'/1'/0']tpubDCuorCpzvYS2LCD75BR46KHE8GdDeg1wsAgNZeNr6DaB5gQK1o14uErKwKLuFmeemkQ6N2m3rNgvctdJLyr7nwu2yia7413Hhg8WWE44cgT/0/*)#5wrnv0xt");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip44<K: DerivableKey<Legacy>>(pub K, pub KeychainKind);

impl<K: DerivableKey<Legacy>> DescriptorTemplate for Bip44<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Pkh(legacy::make_bipxx_private(44, self.0, self.1, network)?).build(network)
    }
}

/// BIP44 public template. Expands to `pkh(key/{0,1}/*)`
///
/// This assumes that the key used has already been derived with `m/44'/0'/0'` for Mainnet or `m/44'/1'/0'` for Testnet.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`Bip44`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip44Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDDDzQ31JkZB7VxUr9bjvBivDdqoFLrDPyLWtLapArAi51ftfmCb2DPxwLQzX65iNcXz1DGaVvyvo6JQ6rTU73r2gqdEo8uov9QKRb7nKCSU")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let wallet = Wallet::new(
///     Bip44Public(key.clone(), fingerprint, KeychainKind::External),
///     Some(Bip44Public(key, fingerprint, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "miNG7dJTzJqNbFS19svRdTCisC65dsubtR");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "pkh([c55b303f/44'/1'/0']tpubDDDzQ31JkZB7VxUr9bjvBivDdqoFLrDPyLWtLapArAi51ftfmCb2DPxwLQzX65iNcXz1DGaVvyvo6JQ6rTU73r2gqdEo8uov9QKRb7nKCSU/0/*)#cfhumdqz");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip44Public<K: DerivableKey<Legacy>>(pub K, pub bip32::Fingerprint, pub KeychainKind);

impl<K: DerivableKey<Legacy>> DescriptorTemplate for Bip44Public<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Pkh(legacy::make_bipxx_public(
            44, self.0, self.1, self.2, network,
        )?)
        .build(network)
    }
}

/// BIP49 template. Expands to `sh(wpkh(key/49'/{0,1}'/0'/{0,1}/*))`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`Bip49Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip49;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let wallet = Wallet::new(
///     Bip49(key.clone(), KeychainKind::External),
///     Some(Bip49(key, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "2N4zkWAoGdUv4NXhSsU8DvS5MB36T8nKHEB");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "sh(wpkh([c55b303f/49'/1'/0']tpubDDYr4kdnZgjjShzYNjZUZXUUtpXaofdkMaipyS8ThEh45qFmhT4hKYways7UXmg6V7het1QiFo9kf4kYUXyDvV4rHEyvSpys9pjCB3pukxi/0/*))#s9vxlc8e");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip49<K: DerivableKey<Segwitv0>>(pub K, pub KeychainKind);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for Bip49<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Wpkh_P2Sh(segwit_v0::make_bipxx_private(49, self.0, self.1, network)?).build(network)
    }
}

/// BIP49 public template. Expands to `sh(wpkh(key/{0,1}/*))`
///
/// This assumes that the key used has already been derived with `m/49'/0'/0'` for Mainnet or `m/49'/1'/0'` for Testnet.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`Bip49`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip49Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC49r947KGK52X5rBWS4BLs5m9SRY3pYHnvRrm7HcybZ3BfdEsGFyzCMzayi1u58eT82ZeyFZwH7DD6Q83E3fM9CpfMtmnTygnLfP59jL9L")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let wallet = Wallet::new(
///     Bip49Public(key.clone(), fingerprint, KeychainKind::External),
///     Some(Bip49Public(key, fingerprint, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "2N3K4xbVAHoiTQSwxkZjWDfKoNC27pLkYnt");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "sh(wpkh([c55b303f/49'/1'/0']tpubDC49r947KGK52X5rBWS4BLs5m9SRY3pYHnvRrm7HcybZ3BfdEsGFyzCMzayi1u58eT82ZeyFZwH7DD6Q83E3fM9CpfMtmnTygnLfP59jL9L/0/*))#3tka9g0q");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip49Public<K: DerivableKey<Segwitv0>>(pub K, pub bip32::Fingerprint, pub KeychainKind);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for Bip49Public<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Wpkh_P2Sh(segwit_v0::make_bipxx_public(
            49, self.0, self.1, self.2, network,
        )?)
        .build(network)
    }
}

/// BIP84 template. Expands to `wpkh(key/84'/{0,1}'/0'/{0,1}/*)`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`Bip84Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip84;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let wallet = Wallet::new(
///     Bip84(key.clone(), KeychainKind::External),
///     Some(Bip84(key, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "tb1qhl85z42h7r4su5u37rvvw0gk8j2t3n9y7zsg4n");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "wpkh([c55b303f/84'/1'/0']tpubDDc5mum24DekpNw92t6fHGp8Gr2JjF9J7i4TZBtN6Vp8xpAULG5CFaKsfugWa5imhrQQUZKXe261asP5koDHo5bs3qNTmf3U3o4v9SaB8gg/0/*)#6kfecsmr");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip84<K: DerivableKey<Segwitv0>>(pub K, pub KeychainKind);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for Bip84<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Wpkh(segwit_v0::make_bipxx_private(84, self.0, self.1, network)?).build(network)
    }
}

/// BIP84 public template. Expands to `wpkh(key/{0,1}/*)`
///
/// This assumes that the key used has already been derived with `m/84'/0'/0'` for Mainnet or `m/84'/1'/0'` for Testnet.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`Bip84`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip84Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let wallet = Wallet::new(
///     Bip84Public(key.clone(), fingerprint, KeychainKind::External),
///     Some(Bip84Public(key, fingerprint, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "tb1qedg9fdlf8cnnqfd5mks6uz5w4kgpk2pr6y4qc7");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "wpkh([c55b303f/84'/1'/0']tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q/0/*)#dhu402yv");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip84Public<K: DerivableKey<Segwitv0>>(pub K, pub bip32::Fingerprint, pub KeychainKind);

impl<K: DerivableKey<Segwitv0>> DescriptorTemplate for Bip84Public<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2Wpkh(segwit_v0::make_bipxx_public(
            84, self.0, self.1, self.2, network,
        )?)
        .build(network)
    }
}

/// BIP86 template. Expands to `tr(key/86'/{0,1}'/0'/{0,1}/*)`
///
/// Since there are hardened derivation steps, this template requires a private derivable key (generally a `xprv`/`tprv`).
///
/// See [`Bip86Public`] for a template that can work with a `xpub`/`tpub`.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip86;
///
/// let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPeZRHk4rTG6orPS2CRNFX3njhUXx5vj9qGog5ZMH4uGReDWN5kCkY3jmWEtWause41CDvBRXD1shKknAMKxT99o9qUTRVC6m")?;
/// let mut wallet = Wallet::new(
///     Bip86(key.clone(), KeychainKind::External),
///     Some(Bip86(key, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "tb1p5unlj09djx8xsjwe97269kqtxqpwpu2epeskgqjfk4lnf69v4tnqpp35qu");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "tr([c55b303f/86'/1'/0']tpubDCiHofpEs47kx358bPdJmTZHmCDqQ8qw32upCSxHrSEdeeBs2T5Mq6QMB2ukeMqhNBiyhosBvJErteVhfURPGXPv3qLJPw5MVpHUewsbP2m/0/*)#dkgvr5hm");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip86<K: DerivableKey<Tap>>(pub K, pub KeychainKind);

impl<K: DerivableKey<Tap>> DescriptorTemplate for Bip86<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2TR(segwit_v1::make_bipxx_private(86, self.0, self.1, network)?).build(network)
    }
}

/// BIP86 public template. Expands to `tr(key/{0,1}/*)`
///
/// This assumes that the key used has already been derived with `m/86'/0'/0'` for Mainnet or `m/86'/1'/0'` for Testnet.
///
/// This template requires the parent fingerprint to populate correctly the metadata of PSBTs.
///
/// See [`Bip86`] for a template that does the full derivation, but requires private data
/// for the key.
///
/// ## Example
///
/// ```
/// # use std::str::FromStr;
/// # use bdk::bitcoin::{PrivateKey, Network};
/// # use bdk::{Wallet,  KeychainKind};
/// # use bdk::database::MemoryDatabase;
/// # use bdk::wallet::AddressIndex::New;
/// use bdk::template::Bip86Public;
///
/// let key = bitcoin::util::bip32::ExtendedPubKey::from_str("tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q")?;
/// let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("c55b303f")?;
/// let mut wallet = Wallet::new(
///     Bip86Public(key.clone(), fingerprint, KeychainKind::External),
///     Some(Bip86Public(key, fingerprint, KeychainKind::Internal)),
///     Network::Testnet,
///     MemoryDatabase::default()
/// )?;
///
/// assert_eq!(wallet.get_address(New)?.to_string(), "tb1pwjp9f2k5n0xq73ecuu0c5njvgqr3vkh7yaylmpqvsuuaafymh0msvcmh37");
/// assert_eq!(wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string(), "tr([c55b303f/86'/1'/0']tpubDC2Qwo2TFsaNC4ju8nrUJ9mqVT3eSgdmy1yPqhgkjwmke3PRXutNGRYAUo6RCHTcVQaDR3ohNU9we59brGHuEKPvH1ags2nevW5opEE9Z5Q/0/*)#2p65srku");
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub struct Bip86Public<K: DerivableKey<Tap>>(pub K, pub bip32::Fingerprint, pub KeychainKind);

impl<K: DerivableKey<Tap>> DescriptorTemplate for Bip86Public<K> {
    fn build(self, network: Network) -> Result<DescriptorTemplateOut, DescriptorError> {
        P2TR(segwit_v1::make_bipxx_public(
            86, self.0, self.1, self.2, network,
        )?)
        .build(network)
    }
}

macro_rules! expand_make_bipxx {
    ( $mod_name:ident, $ctx:ty ) => {
        mod $mod_name {
            use super::*;

            pub(super) fn make_bipxx_private<K: DerivableKey<$ctx>>(
                bip: u32,
                key: K,
                keychain: KeychainKind,
                network: Network,
            ) -> Result<impl IntoDescriptorKey<$ctx>, DescriptorError> {
                let mut derivation_path = Vec::with_capacity(4);
                derivation_path.push(bip32::ChildNumber::from_hardened_idx(bip)?);

                match network {
                    Network::Bitcoin => {
                        derivation_path.push(bip32::ChildNumber::from_hardened_idx(0)?);
                    }
                    _ => {
                        derivation_path.push(bip32::ChildNumber::from_hardened_idx(1)?);
                    }
                }
                derivation_path.push(bip32::ChildNumber::from_hardened_idx(0)?);

                match keychain {
                    KeychainKind::External => {
                        derivation_path.push(bip32::ChildNumber::from_normal_idx(0)?)
                    }
                    KeychainKind::Internal => {
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
                keychain: KeychainKind,
                network: Network,
            ) -> Result<impl IntoDescriptorKey<$ctx>, DescriptorError> {
                let derivation_path: bip32::DerivationPath = match keychain {
                    KeychainKind::External => vec![bip32::ChildNumber::from_normal_idx(0)?].into(),
                    KeychainKind::Internal => vec![bip32::ChildNumber::from_normal_idx(1)?].into(),
                };

                let source_path = bip32::DerivationPath::from(vec![
                    bip32::ChildNumber::from_hardened_idx(bip)?,
                    match network {
                        Network::Bitcoin => bip32::ChildNumber::from_hardened_idx(0)?,
                        _ => bip32::ChildNumber::from_hardened_idx(1)?,
                    },
                    bip32::ChildNumber::from_hardened_idx(0)?,
                ]);

                Ok((key, (parent_fingerprint, source_path), derivation_path))
            }
        }
    };
}

expand_make_bipxx!(legacy, Legacy);
expand_make_bipxx!(segwit_v0, Segwitv0);
expand_make_bipxx!(segwit_v1, Tap);

#[cfg(test)]
mod test {
    // test existing descriptor templates, make sure they are expanded to the right descriptors

    use std::str::FromStr;

    use super::*;
    use crate::descriptor::{DescriptorError, DescriptorMeta};
    use crate::keys::ValidNetworks;
    use assert_matches::assert_matches;
    use miniscript::descriptor::{DescriptorPublicKey, KeyMap};
    use miniscript::Descriptor;

    // BIP44 `pkh(key/44'/{0,1}'/0'/{0,1}/*)`
    #[test]
    fn test_bip44_template_cointype() {
        use bitcoin::util::bip32::ChildNumber::{self, Hardened};

        let xprvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("xprv9s21ZrQH143K2fpbqApQL69a4oKdGVnVN52R82Ft7d1pSqgKmajF62acJo3aMszZb6qQ22QsVECSFxvf9uyxFUvFYQMq3QbtwtRSMjLAhMf").unwrap();
        assert_eq!(Network::Bitcoin, xprvkey.network);
        let xdesc = Bip44(xprvkey, KeychainKind::Internal)
            .build(Network::Bitcoin)
            .unwrap();

        if let ExtendedDescriptor::Pkh(pkh) = xdesc.0 {
            let path: Vec<ChildNumber> = pkh.into_inner().full_derivation_path().into();
            let purpose = path.get(0).unwrap();
            assert_matches!(purpose, Hardened { index: 44 });
            let coin_type = path.get(1).unwrap();
            assert_matches!(coin_type, Hardened { index: 0 });
        }

        let tprvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        assert_eq!(Network::Testnet, tprvkey.network);
        let tdesc = Bip44(tprvkey, KeychainKind::Internal)
            .build(Network::Testnet)
            .unwrap();

        if let ExtendedDescriptor::Pkh(pkh) = tdesc.0 {
            let path: Vec<ChildNumber> = pkh.into_inner().full_derivation_path().into();
            let purpose = path.get(0).unwrap();
            assert_matches!(purpose, Hardened { index: 44 });
            let coin_type = path.get(1).unwrap();
            assert_matches!(coin_type, Hardened { index: 1 });
        }
    }

    // verify template descriptor generates expected address(es)
    fn check(
        desc: Result<(Descriptor<DescriptorPublicKey>, KeyMap, ValidNetworks), DescriptorError>,
        is_witness: bool,
        is_taproot: bool,
        is_fixed: bool,
        network: Network,
        expected: &[&str],
    ) {
        let (desc, _key_map, _networks) = desc.unwrap();
        assert_eq!(desc.is_witness(), is_witness);
        assert_eq!(desc.is_taproot(), is_taproot);
        assert_eq!(!desc.has_wildcard(), is_fixed);
        for i in 0..expected.len() {
            let index = i as u32;
            let child_desc = if !desc.has_wildcard() {
                desc.at_derivation_index(0)
            } else {
                desc.at_derivation_index(index)
            };
            let address = child_desc.address(network).unwrap();
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
            P2Pkh(prvkey).build(Network::Bitcoin),
            false,
            false,
            true,
            Network::Regtest,
            &["mwJ8hxFYW19JLuc65RCTaP4v1rzVU8cVMT"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2Pkh(pubkey).build(Network::Bitcoin),
            false,
            false,
            true,
            Network::Regtest,
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
            P2Wpkh_P2Sh(prvkey).build(Network::Bitcoin),
            true,
            false,
            true,
            Network::Regtest,
            &["2NB4ox5VDRw1ecUv6SnT3VQHPXveYztRqk5"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2Wpkh_P2Sh(pubkey).build(Network::Bitcoin),
            true,
            false,
            true,
            Network::Regtest,
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
            P2Wpkh(prvkey).build(Network::Bitcoin),
            true,
            false,
            true,
            Network::Regtest,
            &["bcrt1q4525hmgw265tl3drrl8jjta7ayffu6jfcwxx9y"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2Wpkh(pubkey).build(Network::Bitcoin),
            true,
            false,
            true,
            Network::Regtest,
            &["bcrt1qngw83fg8dz0k749cg7k3emc7v98wy0c7azaa6h"],
        );
    }

    // P2TR `tr(key)`
    #[test]
    fn test_p2tr_template() {
        let prvkey =
            bitcoin::PrivateKey::from_wif("cTc4vURSzdx6QE6KVynWGomDbLaA75dNALMNyfjh3p8DRRar84Um")
                .unwrap();
        check(
            P2TR(prvkey).build(Network::Bitcoin),
            false,
            true,
            true,
            Network::Regtest,
            &["bcrt1pvjf9t34fznr53u5tqhejz4nr69luzkhlvsdsdfq9pglutrpve2xqnwtkqq"],
        );

        let pubkey = bitcoin::PublicKey::from_str(
            "03a34b99f22c790c4e36b2b3c2c35a36db06226e41c692fc82b8b56ac1c540c5bd",
        )
        .unwrap();
        check(
            P2TR(pubkey).build(Network::Bitcoin),
            false,
            true,
            true,
            Network::Regtest,
            &["bcrt1pw74tdcrxlzn5r8z6ku2vztr86fgq0m245s72mjktf4afwzsf8ugs4evwdf"],
        );
    }

    // BIP44 `pkh(key/44'/0'/0'/{0,1}/*)`
    #[test]
    fn test_bip44_template() {
        let prvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        check(
            Bip44(prvkey, KeychainKind::External).build(Network::Bitcoin),
            false,
            false,
            false,
            Network::Regtest,
            &[
                "n453VtnjDHPyDt2fDstKSu7A3YCJoHZ5g5",
                "mvfrrumXgTtwFPWDNUecBBgzuMXhYM7KRP",
                "mzYvhRAuQqbdSKMVVzXNYyqihgNdRadAUQ",
            ],
        );
        check(
            Bip44(prvkey, KeychainKind::Internal).build(Network::Bitcoin),
            false,
            false,
            false,
            Network::Regtest,
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
            Bip44Public(pubkey, fingerprint, KeychainKind::External).build(Network::Bitcoin),
            false,
            false,
            false,
            Network::Regtest,
            &[
                "miNG7dJTzJqNbFS19svRdTCisC65dsubtR",
                "n2UqaDbCjWSFJvpC84m3FjUk5UaeibCzYg",
                "muCPpS6Ue7nkzeJMWDViw7Lkwr92Yc4K8g",
            ],
        );
        check(
            Bip44Public(pubkey, fingerprint, KeychainKind::Internal).build(Network::Bitcoin),
            false,
            false,
            false,
            Network::Regtest,
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
            Bip49(prvkey, KeychainKind::External).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
            &[
                "2N9bCAJXGm168MjVwpkBdNt6ucka3PKVoUV",
                "2NDckYkqrYyDMtttEav5hB3Bfw9EGAW5HtS",
                "2NAFTVtksF9T4a97M7nyCjwUBD24QevZ5Z4",
            ],
        );
        check(
            Bip49(prvkey, KeychainKind::Internal).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
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
            Bip49Public(pubkey, fingerprint, KeychainKind::External).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
            &[
                "2N3K4xbVAHoiTQSwxkZjWDfKoNC27pLkYnt",
                "2NCTQfJ1sZa3wQ3pPseYRHbaNEpC3AquEfX",
                "2MveFxAuC8BYPzTybx7FxSzW8HSd8ATT4z7",
            ],
        );
        check(
            Bip49Public(pubkey, fingerprint, KeychainKind::Internal).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
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
            Bip84(prvkey, KeychainKind::External).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
            &[
                "bcrt1qkmvk2nadgplmd57ztld8nf8v2yxkzmdvwtjf8s",
                "bcrt1qx0v6zgfwe50m4kqc58cqzcyem7ay2sfl3gvqhp",
                "bcrt1q4h7fq9zhxst6e69p3n882nfj649l7w9g3zccfp",
            ],
        );
        check(
            Bip84(prvkey, KeychainKind::Internal).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
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
            Bip84Public(pubkey, fingerprint, KeychainKind::External).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
            &[
                "bcrt1qedg9fdlf8cnnqfd5mks6uz5w4kgpk2prcdvd0h",
                "bcrt1q3lncdlwq3lgcaaeyruynjnlccr0ve0kakh6ana",
                "bcrt1qt9800y6xl3922jy3uyl0z33jh5wfpycyhcylr9",
            ],
        );
        check(
            Bip84Public(pubkey, fingerprint, KeychainKind::Internal).build(Network::Bitcoin),
            true,
            false,
            false,
            Network::Regtest,
            &[
                "bcrt1qm6wqukenh7guu792lj2njgw9n78cmwsy8xy3z2",
                "bcrt1q694twxtjn4nnrvnyvra769j0a23rllj5c6cgwp",
                "bcrt1qhlac3c5ranv5w5emlnqs7wxhkxt8maelylcarp",
            ],
        );
    }

    // BIP86 `tr(key/86'/0'/0'/{0,1}/*)`
    // Used addresses in test vector in https://github.com/bitcoin/bips/blob/master/bip-0086.mediawiki
    #[test]
    fn test_bip86_template() {
        let prvkey = bitcoin::util::bip32::ExtendedPrivKey::from_str("xprv9s21ZrQH143K3GJpoapnV8SFfukcVBSfeCficPSGfubmSFDxo1kuHnLisriDvSnRRuL2Qrg5ggqHKNVpxR86QEC8w35uxmGoggxtQTPvfUu").unwrap();
        check(
            Bip86(prvkey, KeychainKind::External).build(Network::Bitcoin),
            false,
            true,
            false,
            Network::Bitcoin,
            &[
                "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr",
                "bc1p4qhjn9zdvkux4e44uhx8tc55attvtyu358kutcqkudyccelu0was9fqzwh",
                "bc1p0d0rhyynq0awa9m8cqrcr8f5nxqx3aw29w4ru5u9my3h0sfygnzs9khxz8",
            ],
        );
        check(
            Bip86(prvkey, KeychainKind::Internal).build(Network::Bitcoin),
            false,
            true,
            false,
            Network::Bitcoin,
            &[
                "bc1p3qkhfews2uk44qtvauqyr2ttdsw7svhkl9nkm9s9c3x4ax5h60wqwruhk7",
                "bc1ptdg60grjk9t3qqcqczp4tlyy3z47yrx9nhlrjsmw36q5a72lhdrs9f00nj",
                "bc1pgcwgsu8naxp7xlp5p7ufzs7emtfza2las7r2e7krzjhe5qj5xz2q88kmk5",
            ],
        );
    }

    // BIP86 public `tr(key/{0,1}/*)`
    // Used addresses in test vector in https://github.com/bitcoin/bips/blob/master/bip-0086.mediawiki
    #[test]
    fn test_bip86_public_template() {
        let pubkey = bitcoin::util::bip32::ExtendedPubKey::from_str("xpub6BgBgsespWvERF3LHQu6CnqdvfEvtMcQjYrcRzx53QJjSxarj2afYWcLteoGVky7D3UKDP9QyrLprQ3VCECoY49yfdDEHGCtMMj92pReUsQ").unwrap();
        let fingerprint = bitcoin::util::bip32::Fingerprint::from_str("73c5da0a").unwrap();
        check(
            Bip86Public(pubkey, fingerprint, KeychainKind::External).build(Network::Bitcoin),
            false,
            true,
            false,
            Network::Bitcoin,
            &[
                "bc1p5cyxnuxmeuwuvkwfem96lqzszd02n6xdcjrs20cac6yqjjwudpxqkedrcr",
                "bc1p4qhjn9zdvkux4e44uhx8tc55attvtyu358kutcqkudyccelu0was9fqzwh",
                "bc1p0d0rhyynq0awa9m8cqrcr8f5nxqx3aw29w4ru5u9my3h0sfygnzs9khxz8",
            ],
        );
        check(
            Bip86Public(pubkey, fingerprint, KeychainKind::Internal).build(Network::Bitcoin),
            false,
            true,
            false,
            Network::Bitcoin,
            &[
                "bc1p3qkhfews2uk44qtvauqyr2ttdsw7svhkl9nkm9s9c3x4ax5h60wqwruhk7",
                "bc1ptdg60grjk9t3qqcqczp4tlyy3z47yrx9nhlrjsmw36q5a72lhdrs9f00nj",
                "bc1pgcwgsu8naxp7xlp5p7ufzs7emtfza2las7r2e7krzjhe5qj5xz2q88kmk5",
            ],
        );
    }
}
