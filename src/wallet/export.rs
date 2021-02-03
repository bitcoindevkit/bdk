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
//! # use bdk::database::*;
//! # use bdk::wallet::export::*;
//! # use bdk::*;
//! let import = r#"{
//!     "descriptor": "wpkh([c258d2e4\/84h\/1h\/0h]tpubDD3ynpHgJQW8VvWRzQ5WFDCrs4jqVFGHB3vLC3r49XHJSqP8bHKdK4AriuUKLccK68zfzowx7YhmDN8SiSkgCDENUFx9qVw65YyqM78vyVe\/0\/*)",
//!     "blockheight":1782088,
//!     "label":"testnet"
//! }"#;
//!
//! let import = WalletExport::from_str(import)?;
//! let wallet = Wallet::new_offline(
//!     &import.descriptor(),
//!     import.change_descriptor().as_ref(),
//!     Network::Testnet,
//!     MemoryDatabase::default(),
//! )?;
//! # Ok::<_, bdk::Error>(())
//! ```
//!
//! ### Export a `Wallet`
//! ```
//! # use bitcoin::*;
//! # use bdk::database::*;
//! # use bdk::wallet::export::*;
//! # use bdk::*;
//! let wallet = Wallet::new_offline(
//!     "wpkh([c258d2e4/84h/1h/0h]tpubDD3ynpHgJQW8VvWRzQ5WFDCrs4jqVFGHB3vLC3r49XHJSqP8bHKdK4AriuUKLccK68zfzowx7YhmDN8SiSkgCDENUFx9qVw65YyqM78vyVe/0/*)",
//!     Some("wpkh([c258d2e4/84h/1h/0h]tpubDD3ynpHgJQW8VvWRzQ5WFDCrs4jqVFGHB3vLC3r49XHJSqP8bHKdK4AriuUKLccK68zfzowx7YhmDN8SiSkgCDENUFx9qVw65YyqM78vyVe/1/*)"),
//!     Network::Testnet,
//!     MemoryDatabase::default()
//! )?;
//! let export = WalletExport::export_wallet(&wallet, "exported wallet", true)
//!     .map_err(ToString::to_string)
//!     .map_err(bdk::Error::Generic)?;
//!
//! println!("Exported: {}", export.to_string());
//! # Ok::<_, bdk::Error>(())
//! ```

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use miniscript::descriptor::{ShInner, WshInner};
use miniscript::{Descriptor, DescriptorPublicKey, ScriptContext, Terminal};

use crate::database::BatchDatabase;
use crate::wallet::Wallet;

/// Structure that contains the export of a wallet
///
/// For a usage example see [this module](crate::wallet::export)'s documentation.
#[derive(Debug, Serialize, Deserialize)]
pub struct WalletExport {
    descriptor: String,
    /// Earliest block to rescan when looking for the wallet's transactions
    pub blockheight: u32,
    /// Arbitrary label for the wallet
    pub label: String,
}

impl ToString for WalletExport {
    fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

impl FromStr for WalletExport {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

fn remove_checksum(s: String) -> String {
    s.splitn(2, '#').next().map(String::from).unwrap()
}

impl WalletExport {
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
    pub fn export_wallet<B, D: BatchDatabase>(
        wallet: &Wallet<B, D>,
        label: &str,
        include_blockheight: bool,
    ) -> Result<Self, &'static str> {
        let descriptor = wallet
            .descriptor
            .to_string_with_secret(&wallet.signers.as_key_map(wallet.secp_ctx()));
        let descriptor = remove_checksum(descriptor);
        Self::is_compatible_with_core(&descriptor)?;

        let blockheight = match wallet.database.borrow().iter_txs(false) {
            _ if !include_blockheight => 0,
            Err(_) => 0,
            Ok(txs) => {
                let mut heights = txs
                    .into_iter()
                    .map(|tx| tx.height.unwrap_or(0))
                    .collect::<Vec<_>>();
                heights.sort_unstable();

                *heights.last().unwrap_or(&0)
            }
        };

        let export = WalletExport {
            descriptor,
            label: label.into(),
            blockheight,
        };

        let desc_to_string = |d: &Descriptor<DescriptorPublicKey>| {
            let descriptor =
                d.to_string_with_secret(&wallet.change_signers.as_key_map(wallet.secp_ctx()));
            remove_checksum(descriptor)
        };
        if export.change_descriptor() != wallet.change_descriptor.as_ref().map(desc_to_string) {
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
    use std::str::FromStr;

    use bitcoin::{Network, Txid};

    use super::*;
    use crate::database::{memory::MemoryDatabase, BatchOperations};
    use crate::types::TransactionDetails;
    use crate::wallet::Wallet;

    fn get_test_db() -> MemoryDatabase {
        let mut db = MemoryDatabase::new();
        db.set_tx(&TransactionDetails {
            transaction: None,
            txid: Txid::from_str(
                "4ddff1fa33af17f377f62b72357b43107c19110a8009b36fb832af505efed98a",
            )
            .unwrap(),
            timestamp: 12345678,
            received: 100_000,
            sent: 0,
            fees: 500,
            height: Some(5000),
        })
        .unwrap();

        db
    }

    #[test]
    fn test_export_bip44() {
        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/1/*)";

        let wallet = Wallet::new_offline(
            descriptor,
            Some(change_descriptor),
            Network::Bitcoin,
            get_test_db(),
        )
        .unwrap();
        let export = WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();

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

        let wallet =
            Wallet::new_offline(descriptor, None, Network::Bitcoin, get_test_db()).unwrap();
        WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();
    }

    #[test]
    #[should_panic(expected = "Incompatible change descriptor")]
    fn test_export_incompatible_change() {
        // This wallet has a change descriptor, but the derivation path is not in the "standard"
        // bip44/49/etc format

        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/50'/0'/1/*)";

        let wallet = Wallet::new_offline(
            descriptor,
            Some(change_descriptor),
            Network::Bitcoin,
            get_test_db(),
        )
        .unwrap();
        WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();
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

        let wallet = Wallet::new_offline(
            descriptor,
            Some(change_descriptor),
            Network::Testnet,
            get_test_db(),
        )
        .unwrap();
        let export = WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();

        assert_eq!(export.descriptor(), descriptor);
        assert_eq!(export.change_descriptor(), Some(change_descriptor.into()));
        assert_eq!(export.blockheight, 5000);
        assert_eq!(export.label, "Test Label");
    }

    #[test]
    fn test_export_to_json() {
        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/1/*)";

        let wallet = Wallet::new_offline(
            descriptor,
            Some(change_descriptor),
            Network::Bitcoin,
            get_test_db(),
        )
        .unwrap();
        let export = WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();

        assert_eq!(export.to_string(), "{\"descriptor\":\"wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44\'/0\'/0\'/0/*)\",\"blockheight\":5000,\"label\":\"Test Label\"}");
    }

    #[test]
    fn test_export_from_json() {
        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/1/*)";

        let import_str = "{\"descriptor\":\"wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44\'/0\'/0\'/0/*)\",\"blockheight\":5000,\"label\":\"Test Label\"}";
        let export = WalletExport::from_str(import_str).unwrap();

        assert_eq!(export.descriptor(), descriptor);
        assert_eq!(export.change_descriptor(), Some(change_descriptor.into()));
        assert_eq!(export.blockheight, 5000);
        assert_eq!(export.label, "Test Label");
    }
}
