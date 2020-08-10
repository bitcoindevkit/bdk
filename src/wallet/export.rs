use std::str::FromStr;

use serde::{Deserialize, Serialize};

use miniscript::{Descriptor, ScriptContext, Terminal};

use crate::blockchain::Blockchain;
use crate::database::BatchDatabase;
use crate::wallet::Wallet;

#[derive(Debug, Serialize, Deserialize)]
pub struct WalletExport {
    descriptor: String,
    pub blockheight: u32,
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

impl WalletExport {
    pub fn export_wallet<B: Blockchain, D: BatchDatabase>(
        wallet: &Wallet<B, D>,
        label: &str,
        include_blockheight: bool,
    ) -> Result<Self, &'static str> {
        let descriptor = wallet.descriptor.as_ref().to_string();
        Self::is_compatible_with_core(&descriptor)?;

        let blockheight = match wallet.database.borrow().iter_txs(false) {
            _ if !include_blockheight => 0,
            Err(_) => 0,
            Ok(txs) => {
                let mut heights = txs
                    .into_iter()
                    .map(|tx| tx.height.unwrap_or(0))
                    .collect::<Vec<_>>();
                heights.sort();

                *heights.last().unwrap_or(&0)
            }
        };

        let export = WalletExport {
            descriptor,
            label: label.into(),
            blockheight,
        };

        if export.change_descriptor()
            != wallet
                .change_descriptor
                .as_ref()
                .map(|d| d.as_ref().to_string())
        {
            return Err("Incompatible change descriptor");
        }

        Ok(export)
    }

    fn is_compatible_with_core(descriptor: &str) -> Result<(), &'static str> {
        fn check_ms<Ctx: ScriptContext>(
            terminal: Terminal<String, Ctx>,
        ) -> Result<(), &'static str> {
            if let Terminal::Multi(_, _) = terminal {
                Ok(())
            } else {
                Err("The descriptor contains operators not supported by Bitcoin Core")
            }
        }

        match Descriptor::<String>::from_str(descriptor).map_err(|_| "Invalid descriptor")? {
            Descriptor::Pk(_)
            | Descriptor::Pkh(_)
            | Descriptor::Wpkh(_)
            | Descriptor::ShWpkh(_) => Ok(()),
            Descriptor::Sh(ms) => check_ms(ms.node),
            Descriptor::Wsh(ms) | Descriptor::ShWsh(ms) => check_ms(ms.node),
            _ => Err("The descriptor is not compatible with Bitcoin Core"),
        }
    }

    pub fn descriptor(&self) -> String {
        self.descriptor.clone()
    }

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
    use crate::wallet::{OfflineWallet, Wallet};

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

        let wallet: OfflineWallet<_> = Wallet::new_offline(
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
    #[should_panic(expected = "Incompatible change descriptor")]
    fn test_export_no_change() {
        // This wallet explicitly doesn't have a change descriptor. It should be impossible to
        // export, because exporting this kind of external descriptor normally implies the
        // existence of an internal descriptor

        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";

        let wallet: OfflineWallet<_> =
            Wallet::new_offline(descriptor, None, Network::Testnet, get_test_db()).unwrap();
        WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();
    }

    #[test]
    #[should_panic(expected = "Incompatible change descriptor")]
    fn test_export_incompatible_change() {
        // This wallet has a change descriptor, but the derivation path is not in the "standard"
        // bip44/49/etc format

        let descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/44'/0'/0'/0/*)";
        let change_descriptor = "wpkh(xprv9s21ZrQH143K4CTb63EaMxja1YiTnSEWKMbn23uoEnAzxjdUJRQkazCAtzxGm4LSoTSVTptoV9RbchnKPW9HxKtZumdyxyikZFDLhogJ5Uj/50'/0'/1/*)";

        let wallet: OfflineWallet<_> = Wallet::new_offline(
            descriptor,
            Some(change_descriptor),
            Network::Testnet,
            get_test_db(),
        )
        .unwrap();
        WalletExport::export_wallet(&wallet, "Test Label", true).unwrap();
    }

    #[test]
    fn test_export_multi() {
        let descriptor = "wsh(multi(2,\
                                [73756c7f/48h/0h/0h/2h]tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*,\
                                [f9f62194/48h/0h/0h/2h]tpubDDp3ZSH1yCwusRppH7zgSxq2t1VEUyXSeEp8E5aFS8m43MknUjiF1bSLo3CGWAxbDyhF1XowA5ukPzyJZjznYk3kYi6oe7QxtX2euvKWsk4/0/*,\
                                [c98b1535/48h/0h/0h/2h]tpubDCDi5W4sP6zSnzJeowy8rQDVhBdRARaPhK1axABi8V1661wEPeanpEXj4ZLAUEoikVtoWcyK26TKKJSecSfeKxwHCcRrge9k1ybuiL71z4a/0/*\
                          ))";
        let change_descriptor = "wsh(multi(2,\
                                       [73756c7f/48h/0h/0h/2h]tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/1/*,\
                                       [f9f62194/48h/0h/0h/2h]tpubDDp3ZSH1yCwusRppH7zgSxq2t1VEUyXSeEp8E5aFS8m43MknUjiF1bSLo3CGWAxbDyhF1XowA5ukPzyJZjznYk3kYi6oe7QxtX2euvKWsk4/1/*,\
                                       [c98b1535/48h/0h/0h/2h]tpubDCDi5W4sP6zSnzJeowy8rQDVhBdRARaPhK1axABi8V1661wEPeanpEXj4ZLAUEoikVtoWcyK26TKKJSecSfeKxwHCcRrge9k1ybuiL71z4a/1/*\
                                 ))";

        let wallet: OfflineWallet<_> = Wallet::new_offline(
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

        let wallet: OfflineWallet<_> = Wallet::new_offline(
            descriptor,
            Some(change_descriptor),
            Network::Testnet,
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
