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
//! This modules implements the wallet export format used by [FullyNoded](https://github.com/Fonta1n3/FullyNoded/blob/10b7808c8b929b171cca537fb50522d015168ac9/Docs/Wallets/Wallet-Export-Spec.md).
//!
//! ## Examples
//!
//! ### Import from JSON
//!
//! ```
//! # use std::str::FromStr;
//! # use bitcoin::*;
//! # use bdk::wallet::export::*;
//! # use bdk::*;
//! let import = r#"{
//!     "descriptor": "wpkh([c258d2e4\/84h\/1h\/0h]tpubDD3ynpHgJQW8VvWRzQ5WFDCrs4jqVFGHB3vLC3r49XHJSqP8bHKdK4AriuUKLccK68zfzowx7YhmDN8SiSkgCDENUFx9qVw65YyqM78vyVe\/0\/*)",
//!     "blockheight":1782088,
//!     "label":"testnet"
//! }"#;
//!
//! let import = FullyNodedExport::from_str(import)?;
//! let wallet = Wallet::new_no_persist(
//!     &import.descriptor(),
//!     import.change_descriptor().as_ref(),
//!     Network::Testnet,
//! )?;
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```
//!
//! ### Export a `Wallet`
//! ```
//! # use bitcoin::*;
//! # use bdk::wallet::export::*;
//! # use bdk::*;
//! let wallet = Wallet::new_no_persist(
//!     "wpkh([c258d2e4/84h/1h/0h]tpubDD3ynpHgJQW8VvWRzQ5WFDCrs4jqVFGHB3vLC3r49XHJSqP8bHKdK4AriuUKLccK68zfzowx7YhmDN8SiSkgCDENUFx9qVw65YyqM78vyVe/0/*)",
//!     Some("wpkh([c258d2e4/84h/1h/0h]tpubDD3ynpHgJQW8VvWRzQ5WFDCrs4jqVFGHB3vLC3r49XHJSqP8bHKdK4AriuUKLccK68zfzowx7YhmDN8SiSkgCDENUFx9qVw65YyqM78vyVe/1/*)"),
//!     Network::Testnet,
//! )?;
//! let export = FullyNodedExport::export_wallet(&wallet, "exported wallet", true).unwrap();
//!
//! println!("Exported: {}", export.to_string());
//! # Ok::<_, Box<dyn std::error::Error>>(())
//! ```

use core::str::FromStr;

use alloc::string::{String, ToString};
use serde::{Deserialize, Serialize};

use miniscript::descriptor::{ShInner, WshInner};
use miniscript::{Descriptor, ScriptContext, Terminal};

use crate::types::KeychainKind;
use crate::wallet::Wallet;

/// Alias for [`FullyNodedExport`]
#[deprecated(since = "0.18.0", note = "Please use [`FullyNodedExport`] instead")]
pub type WalletExport = FullyNodedExport;

/// Structure that contains the export of a wallet
///
/// For a usage example see [this module](crate::wallet::export)'s documentation.
#[derive(Debug, Serialize, Deserialize)]
pub struct FullyNodedExport {
    descriptor: String,
    /// Earliest block to rescan when looking for the wallet's transactions
    pub blockheight: u32,
    /// Arbitrary label for the wallet
    pub label: String,
}

impl ToString for FullyNodedExport {
    fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

impl FromStr for FullyNodedExport {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

fn remove_checksum(s: String) -> String {
    s.split_once('#').map(|(a, _)| String::from(a)).unwrap()
}

impl FullyNodedExport {
    /// Export a wallet
    ///
    /// This function returns an error if it determines that the `wallet`'s descriptor(s) are not
    /// supported by Bitcoin Core or don't follow the standard derivation paths defined by BIP44
    /// and others.
    ///
    /// If `include_blockheight` is `true`, this function will look into the `wallet`'s database
    /// for the oldest transaction it knows and use that as the earliest block to rescan.
    ///
    /// If the database is empty or `include_blockheight` is false, the `blockheight` field
    /// returned will be `0`.
    pub fn export_wallet<D>(
        wallet: &Wallet<D>,
        label: &str,
        include_blockheight: bool,
    ) -> Result<Self, &'static str> {
        let descriptor = wallet
            .get_descriptor_for_keychain(KeychainKind::External)
            .to_string_with_secret(
                &wallet
                    .get_signers(KeychainKind::External)
                    .as_key_map(wallet.secp_ctx()),
            );
        let descriptor = remove_checksum(descriptor);
        Self::is_compatible_with_core(&descriptor)?;

        let blockheight = if include_blockheight {
            wallet
                .transactions()
                .next()
                .map_or(0, |canonical_tx| match canonical_tx.observed_as {
                    bdk_chain::ObservedAs::Confirmed(a) => a.confirmation_height,
                    bdk_chain::ObservedAs::Unconfirmed(_) => 0,
                })
        } else {
            0
        };

        let export = FullyNodedExport {
            descriptor,
            label: label.into(),
            blockheight,
        };

        let change_descriptor = match wallet.public_descriptor(KeychainKind::Internal).is_some() {
            false => None,
            true => {
                let descriptor = wallet
                    .get_descriptor_for_keychain(KeychainKind::Internal)
                    .to_string_with_secret(
                        &wallet
                            .get_signers(KeychainKind::Internal)
                            .as_key_map(wallet.secp_ctx()),
                    );
                Some(remove_checksum(descriptor))
            }
        };
        if export.change_descriptor() != change_descriptor {
            return Err("Incompatible change descriptor");
        }

        Ok(export)
    }

    fn is_compatible_with_core(descriptor: &str) -> Result<(), &'static str> {
        fn check_ms<Ctx: ScriptContext>(
            terminal: &Terminal<String, Ctx>,
        ) -> Result<(), &'static str> {
            if let Terminal::Multi(_, _) = terminal {
                Ok(())
            } else {
                Err("The descriptor contains operators not supported by Bitcoin Core")
            }
        }

        // pkh(), wpkh(), sh(wpkh()) are always fine, as well as multi() and sortedmulti()
        match Descriptor::<String>::from_str(descriptor).map_err(|_| "Invalid descriptor")? {
            Descriptor::Pkh(_) | Descriptor::Wpkh(_) => Ok(()),
            Descriptor::Sh(sh) => match sh.as_inner() {
                ShInner::Wpkh(_) => Ok(()),
                ShInner::SortedMulti(_) => Ok(()),
                ShInner::Wsh(wsh) => match wsh.as_inner() {
                    WshInner::SortedMulti(_) => Ok(()),
                    WshInner::Ms(ms) => check_ms(&ms.node),
                },
                ShInner::Ms(ms) => check_ms(&ms.node),
            },
            Descriptor::Wsh(wsh) => match wsh.as_inner() {
                WshInner::SortedMulti(_) => Ok(()),
                WshInner::Ms(ms) => check_ms(&ms.node),
            },
            _ => Err("The descriptor is not compatible with Bitcoin Core"),
        }
    }

    /// Return the external descriptor
    pub fn descriptor(&self) -> String {
        self.descriptor.clone()
    }

    /// Return the internal descriptor, if present
    pub fn change_descriptor(&self) -> Option<String> {
        let replaced = self.descriptor.replace("/0/*", "/1/*");

        if replaced != self.descriptor {
            Some(replaced)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use core::str::FromStr;

    use bdk_chain::{BlockId, ConfirmationTime};
    use bitcoin::hashes::Hash;
    use bitcoin::{BlockHash, Network, Transaction};

    use super::*;
    use crate::wallet::Wallet;

    fn get_test_wallet(
        descriptor: &str,
        change_descriptor: Option<&str>,
        network: Network,
    ) -> Wallet<()> {
        let mut wallet = Wallet::new_no_persist(descriptor, change_descriptor, network).unwrap();
        let transaction = Transaction {
            input: vec![],
            output: vec![],
            version: 0,
            lock_time: bitcoin::PackedLockTime::ZERO,
        };
        wallet
            .insert_checkpoint(BlockId {
                height: 5001,
                hash: BlockHash::all_zeros(),
            })
            .unwrap();
        wallet
            .insert_tx(
                transaction,
                ConfirmationTime::Confirmed {
                    height: 5000,
                    time: 0,
                },
            )
            .unwrap();
        wallet
    }

    #[test]
    fn test_export_bip44() {
        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/1/*)";

        let wallet = get_test_wallet(descriptor, Some(change_descriptor), Network::Bitcoin);
        let export = FullyNodedExport::export_wallet(&wallet, "Test Label", true).unwrap();

        assert_eq!(export.descriptor(), descriptor);
        assert_eq!(export.change_descriptor(), Some(change_descriptor.into()));
        assert_eq!(export.blockheight, 5000);
        assert_eq!(export.label, "Test Label");
    }

    #[test]
    #[should_panic(expected = "Incompatible change descriptor")]
    fn test_export_no_change() {
        // This wallet explicitly doesn't have a change descriptor. It should be impossible to
        // export, because exporting this kind of external descriptor normally implies the
        // existence of an internal descriptor

        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";

        let wallet = get_test_wallet(descriptor, None, Network::Bitcoin);
        FullyNodedExport::export_wallet(&wallet, "Test Label", true).unwrap();
    }

    #[test]
    #[should_panic(expected = "Incompatible change descriptor")]
    fn test_export_incompatible_change() {
        // This wallet has a change descriptor, but the derivation path is not in the "standard"
        // bip44/49/etc format

        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/50'/0'/1/*)";

        let wallet = get_test_wallet(descriptor, Some(change_descriptor), Network::Bitcoin);
        FullyNodedExport::export_wallet(&wallet, "Test Label", true).unwrap();
    }

    #[test]
    fn test_export_multi() {
        let descriptor = "wsh(multi(2,\
                                [73756c7f/48'/0'/0'/2']tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*,\
                                [f9f62194/48'/0'/0'/2']tpubDDp3ZSH1yCwusRppH7zgSxq2t1VEUyXSeEp8E5aFS8m43MknUjiF1bSLo3CGWAxbDyhF1XowA5ukPzyJZjznYk3kYi6oe7QxtX2euvKWsk4/0/*,\
                                [c98b1535/48'/0'/0'/2']tpubDCDi5W4sP6zSnzJeowy8rQDVhBdRARaPhK1axABi8V1661wEPeanpEXj4ZLAUEoikVtoWcyK26TKKJSecSfeKxwHCcRrge9k1ybuiL71z4a/0/*\
                          ))";
        let change_descriptor = "wsh(multi(2,\
                                       [73756c7f/48'/0'/0'/2']tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/1/*,\
                                       [f9f62194/48'/0'/0'/2']tpubDDp3ZSH1yCwusRppH7zgSxq2t1VEUyXSeEp8E5aFS8m43MknUjiF1bSLo3CGWAxbDyhF1XowA5ukPzyJZjznYk3kYi6oe7QxtX2euvKWsk4/1/*,\
                                       [c98b1535/48'/0'/0'/2']tpubDCDi5W4sP6zSnzJeowy8rQDVhBdRARaPhK1axABi8V1661wEPeanpEXj4ZLAUEoikVtoWcyK26TKKJSecSfeKxwHCcRrge9k1ybuiL71z4a/1/*\
                                 ))";

        let wallet = get_test_wallet(descriptor, Some(change_descriptor), Network::Testnet);
        let export = FullyNodedExport::export_wallet(&wallet, "Test Label", true).unwrap();

        assert_eq!(export.descriptor(), descriptor);
        assert_eq!(export.change_descriptor(), Some(change_descriptor.into()));
        assert_eq!(export.blockheight, 5000);
        assert_eq!(export.label, "Test Label");
    }

    #[test]
    fn test_export_to_json() {
        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/1/*)";

        let wallet = get_test_wallet(descriptor, Some(change_descriptor), Network::Bitcoin);
        let export = FullyNodedExport::export_wallet(&wallet, "Test Label", true).unwrap();

        assert_eq!(export.to_string(), "{\"descriptor\":\"wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44\'/0\'/0\'/0/*)\",\"blockheight\":5000,\"label\":\"Test Label\"}");
    }

    #[test]
    fn test_export_from_json() {
        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/1/*)";

        let import_str = "{\"descriptor\":\"wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44\'/0\'/0\'/0/*)\",\"blockheight\":5000,\"label\":\"Test Label\"}";
        let export = FullyNodedExport::from_str(import_str).unwrap();

        assert_eq!(export.descriptor(), descriptor);
        assert_eq!(export.change_descriptor(), Some(change_descriptor.into()));
        assert_eq!(export.blockheight, 5000);
        assert_eq!(export.label, "Test Label");
    }
}
