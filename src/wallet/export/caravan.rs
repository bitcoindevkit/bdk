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

//! Wallet export
//!
//! This modules implements the wallet export format used by Unchained Capitals's [Caravan](https://github.com/unchained-capital/caravan).
//!
//! ## Examples
//!
//! ### Import from JSON
//!
//! ```
//! # use std::str::FromStr;
//! # use bitcoin::*;
//! # use bdk::database::*;
//! # use bdk::wallet::export::caravan::*;
//! # use bdk::*;
//! let import = r#"{
//!   "name": "P2WSH-T",
//!   "addressType": "P2WSH",
//!   "network": "testnet",
//!   "client":  {
//!     "type": "public"
//!   },
//!   "quorum": {
//!     "requiredSigners": 2,
//!     "totalSigners": 2
//!   },
//!   "extendedPublicKeys": [
//!     {
//!         "name": "osw",
//!         "bip32Path": "m/48'/1'/100'/2'",
//!         "xpub": "tpubDFc9Mm4tw6EkgR4YTC1GrU6CGEd9yw7KSBnSssL4LXAXh89D4uMZigRyv3csdXbeU3BhLQc4vWKTLewboA1Pt8Fu6fbHKu81MZ6VGdc32eM",
//!         "xfp" : "f57ec65d"
//!       },
//!     {
//!         "name": "d",
//!         "bip32Path": "m/48'/1'/100'/2'",
//!         "xpub": "tpubDErWN5qfdLwYE94mh12oWr4uURDDNKCjKVhCEcAgZ7jKnnAwq5tcTF2iEk3VuznkJuk2G8SCHft9gS6aKbBd18ptYWPqKLRSTRQY7e2rrDj",
//!         "xfp" : "efa5d916"
//!       }
//!   ],
//!   "startingAddressIndex": 0
//! }"#;
//!
//! let import = CaravanExport::from_str(import)?;
//! let wallet = Wallet::new(
//!     import.descriptor()?,
//!     None,
//!     import.network(),
//!     MemoryDatabase::default(),
//! )?;
//! # Ok::<_, bdk::Error>(())
//! ```
//!
//! ### Export a `Wallet`
//! ```
//! # use bitcoin::*;
//! # use bdk::database::*;
//! # use bdk::wallet::export::caravan::*;
//! # use bdk::*;
//! let wallet = Wallet::new(
//!     "wsh(sortedmulti(2,[f57ec65d/48'/1'/100'/2']tpubDFc9Mm4tw6EkgR4YTC1GrU6CGEd9yw7KSBnSssL4LXAXh89D4uMZigRyv3csdXbeU3BhLQc4vWKTLewboA1Pt8Fu6fbHKu81MZ6VGdc32eM/0/*,[efa5d916/48'/1'/100'/2']tpubDErWN5qfdLwYE94mh12oWr4uURDDNKCjKVhCEcAgZ7jKnnAwq5tcTF2iEk3VuznkJuk2G8SCHft9gS6aKbBd18ptYWPqKLRSTRQY7e2rrDj/0/*))#nv5k65uf",
//!     None,
//!     Network::Testnet,
//!     MemoryDatabase::default()
//! )?;
//!
//! let name = "P2WSH-T".to_string();
//! let client = "public".to_string();
//! let network = wallet.network();
//! let descriptor = wallet.get_descriptor_for_keychain(KeychainKind::External);
//!
//! let export = CaravanExport::export_wallet(&wallet, name, client)?;
//!
//! println!("Exported: {}", export.to_string());
//! # Ok::<_, bdk::Error>(())
//! ```

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::bitcoin::util::bip32::{ChildNumber, DerivationPath, ExtendedPubKey, Fingerprint};
use crate::bitcoin::Network;
use miniscript::{Descriptor, ScriptContext};

use crate::database::BatchDatabase;
use crate::descriptor::{DescriptorError, DescriptorPublicKey, Legacy, Segwitv0};
use crate::error::Error;
use crate::keys::{DerivableKey, DescriptorKey, SortedMultiVec};
use crate::miniscript::descriptor::{ShInner, WshInner};
use crate::miniscript::MiniscriptKey;
use crate::{descriptor, KeychainKind, Wallet};

/// Alias for [`FullyNodedExport`]
#[deprecated(since = "0.18.0", note = "Please use [`FullyNodedExport`] instead")]
pub type WalletExport = FullyNodedExport;

/// Structure that contains the export of a wallet
///
/// For a usage example see [this module](crate::wallet::export::caravan)'s documentation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaravanExport {
    /// Vault name
    pub name: String,
    /// Caravan address type,
    pub address_type: CaravanAddressType,
    /// Caravan network
    network: CaravanNetwork,
    /// Caravan client
    pub client: CaravanClient,
    /// Signing quorum
    pub quorum: Quorum,
    /// Extended public keys
    pub extended_public_keys: Vec<CaravanExtendedPublicKey>,
    /// Starting address index, always 0 when exporting
    pub starting_address_index: u32,
}

impl CaravanExport {
    /// Get the bitcoin network value
    pub fn network(&self) -> Network {
        match self.network {
            CaravanNetwork::Mainnet => Network::Bitcoin,
            CaravanNetwork::Testnet => Network::Testnet,
        }
    }
    /// Get the descriptor value
    pub fn descriptor(&self) -> Result<Descriptor<DescriptorPublicKey>, Error> {
        let required = self.quorum.required_signers;
        let network: Network = self.network();

        let result = match self.address_type {
            CaravanAddressType::P2sh => {
                let keys: Vec<DescriptorKey<Legacy>> = self.descriptor_keys()?;
                descriptor! { sh ( sortedmulti_vec(required, keys) ) }
            }
            CaravanAddressType::P2shP2wsh => {
                let keys: Vec<DescriptorKey<Segwitv0>> = self.descriptor_keys()?;
                descriptor! { sh ( wsh ( sortedmulti_vec(required, keys) ) ) }
            }
            CaravanAddressType::P2wsh => {
                let keys: Vec<DescriptorKey<Segwitv0>> = self.descriptor_keys()?;
                descriptor! { wsh ( sortedmulti_vec(required, keys) ) }
            }
        }
        .map_err(|e| Error::Descriptor(e));

        match result {
            Ok((d, _, n)) => {
                if n.contains(&network) {
                    Ok(d)
                } else {
                    Err(Error::InvalidNetwork {
                        requested: network,
                        found: *n.iter().last().expect("network"),
                    })
                }
            }
            Err(e) => Err(e),
        }
    }

    fn descriptor_keys<Ctx: ScriptContext>(
        &self,
    ) -> Result<Vec<DescriptorKey<Ctx>>, DescriptorError> {
        let result = self
            .extended_public_keys
            .iter()
            .map(|k| {
                let fingerprint = k.xfp;
                let key_path = k.bip32_path.clone();
                let key_source = fingerprint.zip(key_path);
                let derivation_path =
                    DerivationPath::master().child(ChildNumber::Normal { index: 0 });
                k.xpub
                    .into_descriptor_key(key_source, derivation_path)
                    .map_err(|e| DescriptorError::Key(e))
            })
            .collect();
        result
    }

    fn parse_sorted_multi<Pk: MiniscriptKey, Ctx: ScriptContext>(
        sorted_multi: &SortedMultiVec<Pk, Ctx>,
    ) -> (Quorum, &[Pk]) {
        let quorum = Quorum {
            required_signers: sorted_multi.k,
            total_signers: sorted_multi.pks.len(),
        };
        let extended_public_keys = sorted_multi.pks.as_slice();
        (quorum, extended_public_keys)
    }

    /// Export BDK wallet configuration as a Caravan configuration
    pub fn export_wallet<D: BatchDatabase>(
        wallet: &Wallet<D>,
        name: String,
        client_type: String,
    ) -> Result<Self, Error> {
        let network = wallet.network;
        let descriptor = wallet.get_descriptor_for_keychain(KeychainKind::External);
        if wallet.change_descriptor.is_none() {
            Self::export(network, descriptor, name, client_type)
        } else {
            Err(Error::Generic(
                "Can not export a wallet with a change descriptor to Caravan.".to_string(),
            ))
        }
    }

    /// Export BDK wallet network and descriptor as a Caravan configuration
    pub fn export(
        network: Network,
        descriptor: &Descriptor<DescriptorPublicKey>,
        name: String,
        client_type: String,
    ) -> Result<Self, Error> {
        let (address_type, quorum, descriptor_public_keys) = match descriptor {
            Descriptor::Sh(sh) => match sh.as_inner() {
                ShInner::SortedMulti(smv) => {
                    let (quorum, extended_public_keys) = CaravanExport::parse_sorted_multi(smv);
                    Ok((CaravanAddressType::P2sh, quorum, extended_public_keys))
                }
                ShInner::Wsh(wsh) => match wsh.as_inner() {
                    WshInner::SortedMulti(smv) => {
                        let (quorum, extended_public_keys) = CaravanExport::parse_sorted_multi(smv);
                        Ok((CaravanAddressType::P2shP2wsh, quorum, extended_public_keys))
                    }
                    _ => Err(Error::Generic(
                        "Unsupported sh(wsh()) inner descriptor.".to_string(),
                    )),
                },
                _ => Err(Error::Generic(
                    "Unsupported sh() inner descriptor.".to_string(),
                )),
            },
            Descriptor::Wsh(sh) => match sh.as_inner() {
                WshInner::SortedMulti(smv) => {
                    let (quorum, extended_public_keys) = CaravanExport::parse_sorted_multi(smv);
                    Ok((CaravanAddressType::P2wsh, quorum, extended_public_keys))
                }
                _ => Err(Error::Generic(
                    "Unsupported wsh() inner descriptor.".to_string(),
                )),
            },
            _ => Err(Error::Generic(
                "Unsupported top level descriptor.".to_string(),
            )),
        }?;

        let network = match network {
            Network::Bitcoin => CaravanNetwork::Mainnet,
            _ => CaravanNetwork::Testnet,
        };
        let client = CaravanClient { value: client_type };
        let extended_public_keys = descriptor_public_keys
            .iter()
            .map(|k| match k {
                DescriptorPublicKey::SinglePub(_) => {
                    Err(Error::Generic("Unsupported single pub key.".to_string()))
                }
                DescriptorPublicKey::XPub(xpub) => {
                    let mut xfp = None;
                    let mut bip32_path = None;
                    if let Some((s_xfp, s_bip32_path)) = xpub.clone().origin {
                        xfp = Some(s_xfp);
                        bip32_path = Some(s_bip32_path);
                    }
                    Ok(CaravanExtendedPublicKey {
                        name: xpub.xkey.fingerprint().to_string(),
                        bip32_path,
                        xpub: xpub.xkey,
                        xfp,
                    })
                }
            })
            .flatten()
            .collect();

        Ok(Self {
            name,
            address_type,
            network,
            client,
            quorum,
            extended_public_keys,
            starting_address_index: 0,
        })
    }
}

/// The address types supported by Caravan
#[derive(Debug, Serialize, Deserialize)]
pub enum CaravanAddressType {
    /// P2SH
    #[serde(rename = "P2SH")]
    P2sh,
    /// P2SH-P2WSH
    #[serde(rename = "P2SH-P2WSH")]
    P2shP2wsh,
    /// P2WSH
    #[serde(rename = "P2WSH")]
    P2wsh,
}

/// The networks supported by Caravan
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CaravanNetwork {
    Mainnet,
    Testnet,
}

/// A caravan client
#[derive(Debug, Serialize, Deserialize)]
pub struct CaravanClient {
    /// The client type value
    #[serde(rename = "type")]
    value: String,
}

/// The quorum of signers required and total signers
#[derive(Debug, Serialize, Deserialize)]
pub struct Quorum {
    #[serde(rename = "requiredSigners")]
    required_signers: usize,
    #[serde(rename = "totalSigners")]
    total_signers: usize,
}

/// The Caravan extended public key information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaravanExtendedPublicKey {
    name: String,
    #[serde(rename = "bip32Path")]
    bip32_path: Option<DerivationPath>,
    xpub: ExtendedPubKey,
    xfp: Option<Fingerprint>,
}

impl ToString for CaravanExport {
    fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

impl FromStr for CaravanExport {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use crate::bitcoin::Address;

    use super::*;
    use crate::database::memory::MemoryDatabase;
    use crate::wallet::{AddressIndex, Wallet};
    use assert_json_diff::assert_json_include;
    use serde_json::Value;

    fn test_import(import_json: &str, expected_addresses: Vec<&str>) {
        let import = CaravanExport::from_str(import_json).expect("import");
        let descriptor = import.descriptor().expect("descriptor");

        println!("descriptor: {}", descriptor);

        let wallet =
            Wallet::new(descriptor, None, import.network(), MemoryDatabase::new()).expect("wallet");

        for (index, expected_address) in expected_addresses.iter().enumerate() {
            let expected_address = Address::from_str(expected_address).expect("address");
            assert_eq!(
                wallet
                    .get_address(AddressIndex::Peek(index as u32))
                    .unwrap()
                    .address,
                expected_address
            );
        }
    }

    fn test_export(network: Network, descriptor: &str, name: &str, expected_export_json: &str) {
        let wallet =
            Wallet::new(descriptor, None, network, MemoryDatabase::default()).expect("wallet");

        let export = CaravanExport::export_wallet(&wallet, name.to_string(), "public".to_string())
            .expect("export");

        println!("Exported: {}", export.to_string());

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export: Value =
            serde_json::from_str(expected_export_json).expect("expected export");
        assert_json_include!(actual: export, expected: expected_export);
    }

    #[test]
    fn test_import_p2sh_m() {
        let import_json = r#"{
          "name": "P2SH-M",
          "addressType": "P2SH",
          "network": "mainnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "name": "osw",
                "bip32Path": "m/45'/0'/100'",
                "xpub": "xpub6CCHViYn5VzPfSR7baop9FtGcbm3UnqHwa54Z2eNvJnRFCJCdo9HtCYoLJKZCoATMLUowDDA1BMGfQGauY3fDYU3HyMzX4NDkoLYCSkLpbH",
                "xfp" : "f57ec65d"
              },
            {
                "name": "d",
                "bip32Path": "m/45'/0'/100'",
                "xpub": "xpub6Ca5CwTgRASgkXbXE5TeddTP9mPCbYHreCpmGt9dhz9y6femstHGCoFESHHKKRcm414xMKnuLjP9LDS7TwaJC9n5gxua6XB1rwPcC6hqDub",
                "xfp" : "efa5d916"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_import(
            import_json,
            vec![
                "3PiCF26aq57Wo5DJEbFNTVwD1bLCUEpAYZ",
                "3EvHiVyDVoLjeZNMt3v1QTQfs2P4ohVwmg",
                "3PSAx42y6hzWvx2QxQon7CymauWs2SZXuA",
            ],
        );
    }

    #[test]
    fn test_export_p2sh_m() {
        let descriptor = "sh(sortedmulti(2,[f57ec65d/45'/0'/100']xpub6CCHViYn5VzPfSR7baop9FtGcbm3UnqHwa54Z2eNvJnRFCJCdo9HtCYoLJKZCoATMLUowDDA1BMGfQGauY3fDYU3HyMzX4NDkoLYCSkLpbH/0/*,[efa5d916/45'/0'/100']xpub6Ca5CwTgRASgkXbXE5TeddTP9mPCbYHreCpmGt9dhz9y6femstHGCoFESHHKKRcm414xMKnuLjP9LDS7TwaJC9n5gxua6XB1rwPcC6hqDub/0/*))#uxj9xxul";
        let name = "P2SH-M";

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export_json = r#"{
          "name": "P2SH-M",
          "addressType": "P2SH",
          "network": "mainnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "bip32Path": "m/45'/0'/100'",
                "xpub": "xpub6CCHViYn5VzPfSR7baop9FtGcbm3UnqHwa54Z2eNvJnRFCJCdo9HtCYoLJKZCoATMLUowDDA1BMGfQGauY3fDYU3HyMzX4NDkoLYCSkLpbH",
                "xfp" : "f57ec65d"
              },
            {
                "bip32Path": "m/45'/0'/100'",
                "xpub": "xpub6Ca5CwTgRASgkXbXE5TeddTP9mPCbYHreCpmGt9dhz9y6femstHGCoFESHHKKRcm414xMKnuLjP9LDS7TwaJC9n5gxua6XB1rwPcC6hqDub",
                "xfp" : "efa5d916"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_export(Network::Bitcoin, descriptor, name, expected_export_json);
    }

    #[test]
    fn test_import_p2sh_t() {
        let import_json = r#"{
          "name": "P2SH-T",
          "addressType": "P2SH",
          "network": "testnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "name": "dev",
                "bip32Path": "m/45'/1'/100'",
                "xpub": "tpubDDinbKDXyddTUKcX6mv936Ux5utCJteq5S6EEKhfpM8CqN2rMAcccv6GecsB3cPt8eGL4e4K2eaZ9Jis9TGf7mbwBsRTN7ngnFR7yJZxBKC",
                "xfp" : "efa5d916"
              },
            {
                "name": "osw",
                "bip32Path": "m/45'/1'/100'",
                "xpub": "tpubDDQubdBx9cbwQtdcRTisKF7wVCwHgHewhU7wh77VzCi62Q9q81qyQeLoZjKWZ62FnQbWU8k7CuKo2A21pAWaFtPGDHP9WuhtAx4smcCxqn1",
                "xfp" : "f57ec65d"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_import(
            import_json,
            vec![
                "2N5KgAnFFpmk5TRMiCicRZDQS8FFNCKqKf1",
                "2N5hHeNeqk72xkQiHWTHvmpVTpyuKynGrcH",
                "2NC1zVgtFLBfc3UZvnhhjNAF15NmksNCZXe",
            ],
        );
    }

    #[test]
    fn test_export_p2sh_t() {
        let descriptor = "sh(sortedmulti(2,[efa5d916/45'/1'/100']tpubDDinbKDXyddTUKcX6mv936Ux5utCJteq5S6EEKhfpM8CqN2rMAcccv6GecsB3cPt8eGL4e4K2eaZ9Jis9TGf7mbwBsRTN7ngnFR7yJZxBKC/0/*,[f57ec65d/45'/1'/100']tpubDDQubdBx9cbwQtdcRTisKF7wVCwHgHewhU7wh77VzCi62Q9q81qyQeLoZjKWZ62FnQbWU8k7CuKo2A21pAWaFtPGDHP9WuhtAx4smcCxqn1/0/*))#e4qrgzdy";
        let name = "P2SH-T";

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export_json = r#"{
          "name": "P2SH-T",
          "addressType": "P2SH",
          "network": "testnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "bip32Path": "m/45'/1'/100'",
                "xpub": "tpubDDinbKDXyddTUKcX6mv936Ux5utCJteq5S6EEKhfpM8CqN2rMAcccv6GecsB3cPt8eGL4e4K2eaZ9Jis9TGf7mbwBsRTN7ngnFR7yJZxBKC",
                "xfp" : "efa5d916"
              },
            {
                "bip32Path": "m/45'/1'/100'",
                "xpub": "tpubDDQubdBx9cbwQtdcRTisKF7wVCwHgHewhU7wh77VzCi62Q9q81qyQeLoZjKWZ62FnQbWU8k7CuKo2A21pAWaFtPGDHP9WuhtAx4smcCxqn1",
                "xfp" : "f57ec65d"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_export(Network::Testnet, descriptor, name, expected_export_json);
    }

    #[test]
    fn test_import_p2sh_p2wsh_m() {
        let import_json = r#"{
          "name": "P2SH-P2WSH-M",
          "addressType": "P2SH-P2WSH",
          "network": "mainnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "name": "d",
                "bip32Path": "m/48'/0'/100'/1'",
                "xpub": "xpub6EwJjKaiocGvo9f7XSGXGwzo1GLB1URxSZ5Ccp1wqdxNkhrSoqNQkC2CeMsU675urdmFJLHSX62xz56HGcnn6u21wRy6uipovmzaE65PfBp",
                "xfp" : "efa5d916"
              },
            {
                "name": "osw",
                "bip32Path": "m/48'/0'/100'/1'",
                "xpub": "xpub6DcqYQxnbefzEBJF6osEuT5yXoHVZu1YCCsS5YkATvqD2h7tdMBgdBrUXk26FrJwawDGX6fHKPvhhZxKc5b8dPAPb8uANDhsjAPMJqTFDjH",
                "xfp" : "f57ec65d"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_import(
            import_json,
            vec![
                "348PsXezZAHcW7RjmCoMJ8PHWx1QBTXJvm",
                "3GFHyS5GGzTLJaJz6qeSjMrtQGLsbFG4Z8",
                "34Gam7P9rrWwZTeF74WceJ2PGH9XCZTEi6",
            ],
        );
    }

    #[test]
    fn test_export_p2sh_p2wsh_m() {
        let descriptor = "sh(wsh(sortedmulti(2,[efa5d916/48'/0'/100'/1']xpub6EwJjKaiocGvo9f7XSGXGwzo1GLB1URxSZ5Ccp1wqdxNkhrSoqNQkC2CeMsU675urdmFJLHSX62xz56HGcnn6u21wRy6uipovmzaE65PfBp/0/*,[f57ec65d/48'/0'/100'/1']xpub6DcqYQxnbefzEBJF6osEuT5yXoHVZu1YCCsS5YkATvqD2h7tdMBgdBrUXk26FrJwawDGX6fHKPvhhZxKc5b8dPAPb8uANDhsjAPMJqTFDjH/0/*)))#jeqfd8lr";
        let name = "P2SH-P2WSH-M";

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export_json = r#"{
          "name": "P2SH-P2WSH-M",
          "addressType": "P2SH-P2WSH",
          "network": "mainnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "bip32Path": "m/48'/0'/100'/1'",
                "xpub": "xpub6EwJjKaiocGvo9f7XSGXGwzo1GLB1URxSZ5Ccp1wqdxNkhrSoqNQkC2CeMsU675urdmFJLHSX62xz56HGcnn6u21wRy6uipovmzaE65PfBp",
                "xfp" : "efa5d916"
              },
            {
                "bip32Path": "m/48'/0'/100'/1'",
                "xpub": "xpub6DcqYQxnbefzEBJF6osEuT5yXoHVZu1YCCsS5YkATvqD2h7tdMBgdBrUXk26FrJwawDGX6fHKPvhhZxKc5b8dPAPb8uANDhsjAPMJqTFDjH",
                "xfp" : "f57ec65d"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_export(Network::Bitcoin, descriptor, name, expected_export_json);
    }

    #[test]
    fn test_import_p2sh_p2wsh_t() {
        let import_json = r#"{
          "name": "P2SH-P2WSH-T",
          "addressType": "P2SH-P2WSH",
          "network": "testnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "name": "osw",
                "bip32Path": "m/48'/1'/100'/1'",
                "xpub": "tpubDFc9Mm4tw6EkdXuk24MnQYRrDsdKEFh498vFffqa2KJmxytpcHbWrcFYwTKAdLxkSWpadzb5M5VVZ7PDAUjDjymvUmQ7pBbRecz2FM952Am",
                "xfp" : "f57ec65d"
              },
            {
                "name": "d",
                "bip32Path": "m/48'/1'/100'/1'",
                "xpub": "tpubDErWN5qfdLwY9ZJo9HWpxjcuEFuEBVHSbQbPqF35LQr3etWNGirKcgAa93DZ4DmtHm36p2gTf4aj6KybLqHaS3UePM5LtPqtb3d3dYVDs2F",
                "xfp" : "efa5d916"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_import(
            import_json,
            vec![
                "2NDBsV6VBe4d2Ukp2XB644dg2xZ2SuWGkyG",
                "2N2HfmoavC1zjYKxU71Lp1YwCECHXPVKb2Y",
                "2N9g9FZRJ1KUbEvdQ6Mpm5cMGxR3fpM8h5h",
            ],
        );
    }

    #[test]
    fn test_export_p2sh_p2wsh_t() {
        let descriptor = "sh(wsh(sortedmulti(2,[f57ec65d/48'/1'/100'/1']tpubDFc9Mm4tw6EkdXuk24MnQYRrDsdKEFh498vFffqa2KJmxytpcHbWrcFYwTKAdLxkSWpadzb5M5VVZ7PDAUjDjymvUmQ7pBbRecz2FM952Am/0/*,[efa5d916/48'/1'/100'/1']tpubDErWN5qfdLwY9ZJo9HWpxjcuEFuEBVHSbQbPqF35LQr3etWNGirKcgAa93DZ4DmtHm36p2gTf4aj6KybLqHaS3UePM5LtPqtb3d3dYVDs2F/0/*)))#j7jzgtur";
        let name = "P2SH-P2WSH-T";

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export_json = r#"{
          "name": "P2SH-P2WSH-T",
          "addressType": "P2SH-P2WSH",
          "network": "testnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "bip32Path": "m/48'/1'/100'/1'",
                "xpub": "tpubDFc9Mm4tw6EkdXuk24MnQYRrDsdKEFh498vFffqa2KJmxytpcHbWrcFYwTKAdLxkSWpadzb5M5VVZ7PDAUjDjymvUmQ7pBbRecz2FM952Am",
                "xfp" : "f57ec65d"
              },
            {
                "bip32Path": "m/48'/1'/100'/1'",
                "xpub": "tpubDErWN5qfdLwY9ZJo9HWpxjcuEFuEBVHSbQbPqF35LQr3etWNGirKcgAa93DZ4DmtHm36p2gTf4aj6KybLqHaS3UePM5LtPqtb3d3dYVDs2F",
                "xfp" : "efa5d916"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_export(Network::Testnet, descriptor, name, expected_export_json);
    }

    #[test]
    fn test_import_p2wsh_m() {
        let import_json = r#"{
          "name": "P2WSH-M",
          "addressType": "P2WSH",
          "network": "mainnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "name": "d",
                "bip32Path": "m/48'/0'/100'/2'",
                "xpub": "xpub6EwJjKaiocGvqSuM2jRZSuQ9HEddiFUFu9RdjE47zG7kXVNDQpJ3GyvskwYiLmvU4SBTNZyv8UH53QcmFEE23YwozE61V3dwzZJEFQr6H2b",
                "xfp" : "efa5d916"
              },
            {
                "name": "osw",
                "bip32Path": "m/48'/0'/100'/2'",
                "xpub": "xpub6DcqYQxnbefzFkaRBK63FSE2GzNuNnNhFGw1xV9RioVG7av6r3JDf1aELqBSq5gt5487CtNxvVtaiJjQU2HQWzgG5NzLyTPbYav6otW8qEc",
                "xfp" : "f57ec65d"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_import(
            import_json,
            vec![
                "bc1qf9asympax4r6xrndsqrw8p0qxe40tm9zkk69tkrc8p6eg8ju075sjeekkt",
                "bc1q2dexslsgvj4w2adf2lltthglkchmh3d2qvyrtdrece6lfr5tl4cq382unz",
                "bc1q3kwd3zfaa90r20nvm2u3zxtw9c8cf5x4a4ecgw2y7pf59pnpmxns9keq9w",
            ],
        );
    }

    #[test]
    fn test_export_p2wsh_m() {
        let descriptor = "wsh(sortedmulti(2,[efa5d916/48'/0'/100'/2']xpub6EwJjKaiocGvqSuM2jRZSuQ9HEddiFUFu9RdjE47zG7kXVNDQpJ3GyvskwYiLmvU4SBTNZyv8UH53QcmFEE23YwozE61V3dwzZJEFQr6H2b/0/*,[f57ec65d/48'/0'/100'/2']xpub6DcqYQxnbefzFkaRBK63FSE2GzNuNnNhFGw1xV9RioVG7av6r3JDf1aELqBSq5gt5487CtNxvVtaiJjQU2HQWzgG5NzLyTPbYav6otW8qEc/0/*))#decr929e";
        let name = "P2WSH-M";

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export_json = r#"{
          "name": "P2WSH-M",
          "addressType": "P2WSH",
          "network": "mainnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "bip32Path": "m/48'/0'/100'/2'",
                "xpub": "xpub6EwJjKaiocGvqSuM2jRZSuQ9HEddiFUFu9RdjE47zG7kXVNDQpJ3GyvskwYiLmvU4SBTNZyv8UH53QcmFEE23YwozE61V3dwzZJEFQr6H2b",
                "xfp" : "efa5d916"
              },
            {
                "bip32Path": "m/48'/0'/100'/2'",
                "xpub": "xpub6DcqYQxnbefzFkaRBK63FSE2GzNuNnNhFGw1xV9RioVG7av6r3JDf1aELqBSq5gt5487CtNxvVtaiJjQU2HQWzgG5NzLyTPbYav6otW8qEc",
                "xfp" : "f57ec65d"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_export(Network::Bitcoin, descriptor, name, expected_export_json);
    }

    #[test]
    fn test_import_p2wsh_t() {
        let import_json = r#"{
          "name": "P2WSH-T",
          "addressType": "P2WSH",
          "network": "testnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "name": "osw",
                "bip32Path": "m/48'/1'/100'/2'",
                "xpub": "tpubDFc9Mm4tw6EkgR4YTC1GrU6CGEd9yw7KSBnSssL4LXAXh89D4uMZigRyv3csdXbeU3BhLQc4vWKTLewboA1Pt8Fu6fbHKu81MZ6VGdc32eM",
                "xfp" : "f57ec65d"
              },
            {
                "name": "d",
                "bip32Path": "m/48'/1'/100'/2'",
                "xpub": "tpubDErWN5qfdLwYE94mh12oWr4uURDDNKCjKVhCEcAgZ7jKnnAwq5tcTF2iEk3VuznkJuk2G8SCHft9gS6aKbBd18ptYWPqKLRSTRQY7e2rrDj",
                "xfp" : "efa5d916"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_import(
            import_json,
            vec![
                "tb1qhgj3fnwn50pq966rjnj4pg8uz9ktsd8nge32qxd73ffvvg636p5q54g7m0",
                "tb1q4ka64s7fcdv8ms7xs6j2w35dz8t7n0zd450lgsny73jvg8lpyqfqr9n037",
                "tb1q8fglyvwtlr5t427cqn898jc9vrqxkc43522tpxjaupmn8ewu9sushz86gf",
            ],
        );
    }

    #[test]
    fn test_export_p2wsh_t() {
        let descriptor = "wsh(sortedmulti(2,[f57ec65d/48'/1'/100'/2']tpubDFc9Mm4tw6EkgR4YTC1GrU6CGEd9yw7KSBnSssL4LXAXh89D4uMZigRyv3csdXbeU3BhLQc4vWKTLewboA1Pt8Fu6fbHKu81MZ6VGdc32eM/0/*,[efa5d916/48'/1'/100'/2']tpubDErWN5qfdLwYE94mh12oWr4uURDDNKCjKVhCEcAgZ7jKnnAwq5tcTF2iEk3VuznkJuk2G8SCHft9gS6aKbBd18ptYWPqKLRSTRQY7e2rrDj/0/*))#nv5k65uf";
        let name = "P2WSH-T";

        // NOTE: .extendedPublicKeys[].name fields are set to key hash and are not expected
        let expected_export_json = r#"{
          "name": "P2WSH-T",
          "addressType": "P2WSH",
          "network": "testnet",
          "client":  {
            "type": "public"
          },
          "quorum": {
            "requiredSigners": 2,
            "totalSigners": 2
          },
          "extendedPublicKeys": [
            {
                "bip32Path": "m/48'/1'/100'/2'",
                "xpub": "tpubDFc9Mm4tw6EkgR4YTC1GrU6CGEd9yw7KSBnSssL4LXAXh89D4uMZigRyv3csdXbeU3BhLQc4vWKTLewboA1Pt8Fu6fbHKu81MZ6VGdc32eM",
                "xfp" : "f57ec65d"
              },
            {
                "bip32Path": "m/48'/1'/100'/2'",
                "xpub": "tpubDErWN5qfdLwYE94mh12oWr4uURDDNKCjKVhCEcAgZ7jKnnAwq5tcTF2iEk3VuznkJuk2G8SCHft9gS6aKbBd18ptYWPqKLRSTRQY7e2rrDj",
                "xfp" : "efa5d916"
              }
          ],
          "startingAddressIndex": 0
        }"#;

        test_export(Network::Testnet, descriptor, name, expected_export_json);
    }
}
