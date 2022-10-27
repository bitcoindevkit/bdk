use std::collections::HashMap;

use bitcoin::consensus::deserialize;
use bitcoin::consensus::encode::serialize;
use bitcoin::hash_types::Txid;
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::{OutPoint, Script, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use gloo_storage::{LocalStorage, Storage};

use crate::database::memory::MapKey;
use crate::database::{BatchDatabase, BatchOperations, ConfigurableDatabase, Database, SyncTime};
use crate::{Error, KeychainKind, LocalUtxo, TransactionDetails};

#[derive(Debug, Default)]
pub struct LocalStorageDatabase {}

impl ConfigurableDatabase for LocalStorageDatabase {
    type Config = ();

    fn from_config(_config: &Self::Config) -> Result<Self, Error> {
        Ok(LocalStorageDatabase::default())
    }
}

impl LocalStorageDatabase {
    // A wrapper for LocalStorage::set that converts the error to Error
    fn set<T>(&self, key: impl AsRef<str>, value: T) -> Result<(), Error>
    where
        T: Serialize,
    {
        LocalStorage::set(key, value).map_err(|_| Error::Generic("Storage error".to_string()))
    }

    // mostly a copy of LocalStorage::get_all()
    fn scan_prefix(&self, prefix: Vec<u8>) -> Map<String, Value> {
        let local_storage = LocalStorage::raw();
        let length = LocalStorage::length();
        let mut map = Map::with_capacity(length as usize);
        for index in 0..length {
            let key_opt: Option<String> = local_storage.key(index).unwrap();

            if let Some(key) = key_opt {
                if key.starts_with(&prefix.to_hex()) {
                    let value: Value = LocalStorage::get(&key).unwrap();
                    map.insert(key, value);
                }
            }
        }

        map
    }
}

impl BatchOperations for LocalStorageDatabase {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        keychain: KeychainKind,
        path: u32,
    ) -> Result<(), Error> {
        let key = MapKey::Path((Some(keychain), Some(path))).as_map_key();
        self.set(key, script.clone())?;

        let key = MapKey::Script(Some(script)).as_map_key();
        let spk_info = ScriptPubKeyInfo { keychain, path };
        self.set(key, spk_info)?;

        Ok(())
    }

    fn set_utxo(&mut self, utxo: &LocalUtxo) -> Result<(), Error> {
        let key = MapKey::Utxo(Some(&utxo.outpoint)).as_map_key();
        self.set(key, utxo)?;

        Ok(())
    }
    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
        let key = MapKey::RawTx(Some(&transaction.txid())).as_map_key();
        self.set(key, transaction.clone())?;

        Ok(())
    }
    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error> {
        let key = MapKey::Transaction(Some(&transaction.txid)).as_map_key();

        // insert the raw_tx if present
        if let Some(ref tx) = transaction.transaction {
            self.set_raw_tx(tx)?;
        }

        // remove the raw tx from the serialized version
        let mut transaction = transaction.clone();
        transaction.transaction = None;

        self.set(key, transaction)?;

        Ok(())
    }
    fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), Error> {
        let key = MapKey::LastIndex(keychain).as_map_key();
        self.set(key, value)?;

        Ok(())
    }
    fn set_sync_time(&mut self, data: SyncTime) -> Result<(), Error> {
        let key = MapKey::SyncTime.as_map_key();
        self.set(key, data)?;

        Ok(())
    }

    fn del_script_pubkey_from_path(
        &mut self,
        keychain: KeychainKind,
        path: u32,
    ) -> Result<Option<Script>, Error> {
        let key = MapKey::Path((Some(keychain), Some(path))).as_map_key();
        let res: Option<Script> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        Ok(res)
    }
    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        let key = MapKey::Script(Some(script)).as_map_key();
        let res: Option<ScriptPubKeyInfo> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        match res {
            None => Ok(None),
            Some(spk_info) => Ok(Some((spk_info.keychain, spk_info.path))),
        }
    }
    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        let key = MapKey::Utxo(Some(outpoint)).as_map_key();
        let res: Option<LocalUtxo> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        Ok(res)
    }
    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let key = MapKey::RawTx(Some(txid)).as_map_key();
        let res: Option<Transaction> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        Ok(res)
    }
    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, Error> {
        let raw_tx = if include_raw {
            self.del_raw_tx(txid)?
        } else {
            None
        };

        let key = MapKey::Transaction(Some(txid)).as_map_key();
        let res: Option<TransactionDetails> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        match res {
            None => Ok(None),
            Some(mut val) => {
                val.transaction = raw_tx;

                Ok(Some(val))
            }
        }
    }
    fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        let key = MapKey::LastIndex(keychain).as_map_key();
        let res: Option<u32> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        Ok(res)
    }
    fn del_sync_time(&mut self) -> Result<Option<SyncTime>, Error> {
        let key = MapKey::SyncTime.as_map_key();
        let res: Option<SyncTime> = LocalStorage::get(&key).ok();
        LocalStorage::delete(&key);

        Ok(res)
    }
}

impl Database for LocalStorageDatabase {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        keychain: KeychainKind,
        bytes: B,
    ) -> Result<(), Error> {
        let key = MapKey::DescriptorChecksum(keychain).as_map_key();

        let prev = LocalStorage::get::<Vec<u8>>(&key).ok();
        if let Some(val) = prev {
            if val == bytes.as_ref().to_vec() {
                Ok(())
            } else {
                Err(Error::ChecksumMismatch)
            }
        } else {
            self.set(key, bytes.as_ref().to_vec())?;
            Ok(())
        }
    }

    fn iter_script_pubkeys(&self, keychain: Option<KeychainKind>) -> Result<Vec<Script>, Error> {
        let key = MapKey::Path((keychain, None)).as_map_key();
        self.scan_prefix(key)
            .into_iter()
            .map(|(_, value)| -> Result<_, Error> {
                let str_opt = value.as_str();

                match str_opt {
                    Some(str) => Script::from_hex(str)
                        .map_err(|_| Error::Generic(String::from("Error decoding json"))),
                    None => Err(Error::Generic(String::from("Error decoding json"))),
                }
            })
            .collect()
    }

    fn iter_utxos(&self) -> Result<Vec<LocalUtxo>, Error> {
        let key = MapKey::Utxo(None).as_map_key();
        self.scan_prefix(key)
            .into_iter()
            .map(|(_, value)| -> Result<_, Error> {
                let utxo: LocalUtxo = Deserialize::deserialize(value)?;
                Ok(utxo)
            })
            .collect()
    }

    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error> {
        let key = MapKey::RawTx(None).as_map_key();
        self.scan_prefix(key)
            .into_iter()
            .map(|(_, value)| -> Result<_, Error> {
                let tx: Transaction = Deserialize::deserialize(value)?;
                Ok(tx)
            })
            .collect()
    }

    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        let key = MapKey::Transaction(None).as_map_key();
        self.scan_prefix(key)
            .into_iter()
            .map(|(key, value)| -> Result<_, Error> {
                let mut tx_details: TransactionDetails = Deserialize::deserialize(value)?;
                if include_raw {
                    // first byte is prefix for the map, need to drop it
                    let rm_prefix_opt = key.get(2..key.len());
                    match rm_prefix_opt {
                        Some(rm_prefix) => {
                            let k_bytes = Vec::from_hex(rm_prefix)?;
                            let txid = deserialize(k_bytes.as_slice())?;
                            tx_details.transaction = self.get_raw_tx(&txid)?;
                            Ok(tx_details)
                        }
                        None => Err(Error::Generic(String::from("Error parsing txid from json"))),
                    }
                } else {
                    Ok(tx_details)
                }
            })
            .collect()
    }

    fn get_script_pubkey_from_path(
        &self,
        keychain: KeychainKind,
        path: u32,
    ) -> Result<Option<Script>, Error> {
        let key = MapKey::Path((Some(keychain), Some(path))).as_map_key();
        Ok(LocalStorage::get::<Script>(&key).ok())
    }

    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        let key = MapKey::Script(Some(script)).as_map_key();
        Ok(LocalStorage::get::<ScriptPubKeyInfo>(&key)
            .ok()
            .map(|info| (info.keychain, info.path)))
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        let key = MapKey::Utxo(Some(outpoint)).as_map_key();
        let res: Option<LocalUtxo> = LocalStorage::get(key).ok();

        Ok(res)
    }

    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let key = MapKey::RawTx(Some(txid)).as_map_key();
        Ok(LocalStorage::get::<Transaction>(&key).ok())
    }

    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        let key = MapKey::Transaction(Some(txid)).as_map_key();
        Ok(LocalStorage::get::<TransactionDetails>(&key)
            .ok()
            .map(|mut txdetails| {
                if include_raw {
                    txdetails.transaction = self.get_raw_tx(txid).unwrap();
                }

                txdetails
            }))
    }

    fn get_last_index(&self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        let key = MapKey::LastIndex(keychain).as_map_key();
        Ok(LocalStorage::get::<u32>(key).ok())
    }

    fn get_sync_time(&self) -> Result<Option<SyncTime>, Error> {
        let key = MapKey::SyncTime.as_map_key();
        Ok(LocalStorage::get::<SyncTime>(key).ok())
    }

    // inserts 0 if not present
    fn increment_last_index(&mut self, keychain: KeychainKind) -> Result<u32, Error> {
        let key = MapKey::LastIndex(keychain).as_map_key();
        let current_opt = LocalStorage::get::<u32>(&key).ok();
        let value = current_opt.map(|s| s + 1).unwrap_or_else(|| 0);
        self.set(key, value)?;

        Ok(value)
    }
}

impl BatchDatabase for LocalStorageDatabase {
    type Batch = Self;

    fn begin_batch(&self) -> Self::Batch {
        LocalStorageDatabase::default()
    }

    fn commit_batch(&mut self, mut _batch: Self::Batch) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bitcoin::consensus::encode::deserialize;
    use bitcoin::consensus::serialize;
    use bitcoin::hashes::hex::*;
    use bitcoin::*;

    use database::{BatchDatabase, Database, SyncTime};
    use wasm_bindgen_test::{wasm_bindgen_test as test, wasm_bindgen_test_configure};
    use {BlockTime, KeychainKind, LocalUtxo, TransactionDetails};

    use crate::database::test::*;
    use crate::localstorage::LocalStorageDatabase;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn get_tree() -> LocalStorageDatabase {
        LocalStorage::clear();
        LocalStorageDatabase::default()
    }

    #[test]
    fn script_pubkey_test() {
        test_script_pubkey(get_tree());
    }

    // fixme, we don't actually batch, is that okay?
    // #[test]
    // fn script_pubkey_test_batch() {
    //     test_batch_script_pubkey(get_tree());
    // }

    #[test]
    fn script_pubkey_test_iter() {
        test_iter_script_pubkey(get_tree());
    }

    #[test]
    fn script_pubkey_test_del() {
        test_del_script_pubkey(get_tree());
    }

    #[test]
    fn utxo_test() {
        test_utxo(get_tree());
    }

    #[test]
    fn raw_tx_test() {
        test_raw_tx(get_tree());
    }

    #[test]
    fn tx_test() {
        test_tx(get_tree());
    }

    #[test]
    fn last_index_test() {
        test_last_index(get_tree());
    }

    #[test]
    fn sync_time_test() {
        test_sync_time(get_tree());
    }

    #[test]
    fn iter_raw_txs_test() {
        test_iter_raw_txs(get_tree());
    }

    #[test]
    fn list_txs_test() {
        test_list_transaction(get_tree());
    }

    #[test]
    fn del_path_from_script_pubkey_test() {
        test_del_path_from_script_pubkey(get_tree());
    }

    #[test]
    fn iter_script_pubkeys_test() {
        test_iter_script_pubkeys(get_tree());
    }

    #[test]
    fn del_utxo_test() {
        test_del_utxo(get_tree());
    }

    #[test]
    fn del_raw_tx_test() {
        test_del_raw_tx(get_tree());
    }

    #[test]
    fn del_tx_test() {
        test_del_tx(get_tree());
    }

    #[test]
    fn del_last_index_test() {
        test_del_last_index(get_tree());
    }

    #[test]
    fn check_descriptor_checksum_test() {
        test_check_descriptor_checksum(get_tree());
    }
}
