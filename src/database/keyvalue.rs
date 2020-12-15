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

use std::convert::TryInto;

use sled::{Batch, Tree};

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hash_types::Txid;
use bitcoin::{OutPoint, Script, Transaction};

use crate::database::memory::MapKey;
use crate::database::{BatchDatabase, BatchOperations, Database};
use crate::error::Error;
use crate::types::*;

macro_rules! impl_batch_operations {
    ( { $($after_insert:tt)* }, $process_delete:ident ) => {
        fn set_script_pubkey(&mut self, script: &Script, keychain: KeychainKind, path: u32) -> Result<(), Error> {
            let key = MapKey::Path((Some(keychain), Some(path))).as_map_key();
            self.insert(key, serialize(script))$($after_insert)*;

            let key = MapKey::Script(Some(script)).as_map_key();
            let value = json!({
                "t": keychain,
                "p": path,
            });
            self.insert(key, serde_json::to_vec(&value)?)$($after_insert)*;

            Ok(())
        }

        fn set_utxo(&mut self, utxo: &UTXO) -> Result<(), Error> {
            let key = MapKey::UTXO(Some(&utxo.outpoint)).as_map_key();
            let value = json!({
                "t": utxo.txout,
                "i": utxo.keychain,
            });
            self.insert(key, serde_json::to_vec(&value)?)$($after_insert)*;

            Ok(())
        }

        fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
            let key = MapKey::RawTx(Some(&transaction.txid())).as_map_key();
            let value = serialize(transaction);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error> {
            let key = MapKey::Transaction(Some(&transaction.txid)).as_map_key();

            // remove the raw tx from the serialized version
            let mut value = serde_json::to_value(transaction)?;
            value["transaction"] = serde_json::Value::Null;
            let value = serde_json::to_vec(&value)?;

            self.insert(key, value)$($after_insert)*;

            // insert the raw_tx if present
            if let Some(ref tx) = transaction.transaction {
                self.set_raw_tx(tx)?;
            }

            Ok(())
        }

        fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), Error> {
            let key = MapKey::LastIndex(keychain).as_map_key();
            self.insert(key, &value.to_be_bytes())$($after_insert)*;

            Ok(())
        }

        fn del_script_pubkey_from_path(&mut self, keychain: KeychainKind, path: u32) -> Result<Option<Script>, Error> {
            let key = MapKey::Path((Some(keychain), Some(path))).as_map_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            Ok(res.map_or(Ok(None), |x| Some(deserialize(&x)).transpose())?)
        }

        fn del_path_from_script_pubkey(&mut self, script: &Script) -> Result<Option<(KeychainKind, u32)>, Error> {
            let key = MapKey::Script(Some(script)).as_map_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            match res {
                None => Ok(None),
                Some(b) => {
                    let mut val: serde_json::Value = serde_json::from_slice(&b)?;
                    let st = serde_json::from_value(val["t"].take())?;
                    let path = serde_json::from_value(val["p"].take())?;

                    Ok(Some((st, path)))
                }
            }
        }

        fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error> {
            let key = MapKey::UTXO(Some(outpoint)).as_map_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            match res {
                None => Ok(None),
                Some(b) => {
                    let mut val: serde_json::Value = serde_json::from_slice(&b)?;
                    let txout = serde_json::from_value(val["t"].take())?;
                    let keychain = serde_json::from_value(val["i"].take())?;

                    Ok(Some(UTXO { outpoint: outpoint.clone(), txout, keychain }))
                }
            }
        }

        fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
            let key = MapKey::RawTx(Some(txid)).as_map_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            Ok(res.map_or(Ok(None), |x| Some(deserialize(&x)).transpose())?)
        }

        fn del_tx(&mut self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
            let raw_tx = if include_raw {
                self.del_raw_tx(txid)?
            } else {
                None
            };

            let key = MapKey::Transaction(Some(txid)).as_map_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            match res {
                None => Ok(None),
                Some(b) => {
                    let mut val: TransactionDetails = serde_json::from_slice(&b)?;
                    val.transaction = raw_tx;

                    Ok(Some(val))
                }
            }
        }

        fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
            let key = MapKey::LastIndex(keychain).as_map_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            match res {
                None => Ok(None),
                Some(b) => {
                    let array: [u8; 4] = b.as_ref().try_into().map_err(|_| Error::InvalidU32Bytes(b.to_vec()))?;
                    let val = u32::from_be_bytes(array);
                    Ok(Some(val))
                }
            }
        }
    }
}

macro_rules! process_delete_tree {
    ($res:expr) => {
        $res?
    };
}
impl BatchOperations for Tree {
    impl_batch_operations!({?}, process_delete_tree);
}

macro_rules! process_delete_batch {
    ($res:expr) => {
        None as Option<sled::IVec>
    };
}
#[allow(unused_variables)]
impl BatchOperations for Batch {
    impl_batch_operations!({}, process_delete_batch);
}

impl Database for Tree {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        keychain: KeychainKind,
        bytes: B,
    ) -> Result<(), Error> {
        let key = MapKey::DescriptorChecksum(keychain).as_map_key();

        let prev = self.get(&key)?.map(|x| x.to_vec());
        if let Some(val) = prev {
            if val == bytes.as_ref() {
                Ok(())
            } else {
                Err(Error::ChecksumMismatch)
            }
        } else {
            self.insert(&key, bytes.as_ref())?;
            Ok(())
        }
    }

    fn iter_script_pubkeys(&self, keychain: Option<KeychainKind>) -> Result<Vec<Script>, Error> {
        let key = MapKey::Path((keychain, None)).as_map_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (_, v) = x?;
                Ok(deserialize(&v)?)
            })
            .collect()
    }

    fn iter_utxos(&self) -> Result<Vec<UTXO>, Error> {
        let key = MapKey::UTXO(None).as_map_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (k, v) = x?;
                let outpoint = deserialize(&k[1..])?;

                let mut val: serde_json::Value = serde_json::from_slice(&v)?;
                let txout = serde_json::from_value(val["t"].take())?;
                let keychain = serde_json::from_value(val["i"].take())?;

                Ok(UTXO {
                    outpoint,
                    txout,
                    keychain,
                })
            })
            .collect()
    }

    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error> {
        let key = MapKey::RawTx(None).as_map_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (_, v) = x?;
                Ok(deserialize(&v)?)
            })
            .collect()
    }

    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        let key = MapKey::Transaction(None).as_map_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (k, v) = x?;
                let mut txdetails: TransactionDetails = serde_json::from_slice(&v)?;
                if include_raw {
                    let txid = deserialize(&k[1..])?;
                    txdetails.transaction = self.get_raw_tx(&txid)?;
                }

                Ok(txdetails)
            })
            .collect()
    }

    fn get_script_pubkey_from_path(
        &self,
        keychain: KeychainKind,
        path: u32,
    ) -> Result<Option<Script>, Error> {
        let key = MapKey::Path((Some(keychain), Some(path))).as_map_key();
        Ok(self.get(key)?.map(|b| deserialize(&b)).transpose()?)
    }

    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        let key = MapKey::Script(Some(script)).as_map_key();
        self.get(key)?
            .map(|b| -> Result<_, Error> {
                let mut val: serde_json::Value = serde_json::from_slice(&b)?;
                let st = serde_json::from_value(val["t"].take())?;
                let path = serde_json::from_value(val["p"].take())?;

                Ok((st, path))
            })
            .transpose()
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error> {
        let key = MapKey::UTXO(Some(outpoint)).as_map_key();
        self.get(key)?
            .map(|b| -> Result<_, Error> {
                let mut val: serde_json::Value = serde_json::from_slice(&b)?;
                let txout = serde_json::from_value(val["t"].take())?;
                let keychain = serde_json::from_value(val["i"].take())?;

                Ok(UTXO {
                    outpoint: *outpoint,
                    txout,
                    keychain,
                })
            })
            .transpose()
    }

    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let key = MapKey::RawTx(Some(txid)).as_map_key();
        Ok(self.get(key)?.map(|b| deserialize(&b)).transpose()?)
    }

    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        let key = MapKey::Transaction(Some(txid)).as_map_key();
        self.get(key)?
            .map(|b| -> Result<_, Error> {
                let mut txdetails: TransactionDetails = serde_json::from_slice(&b)?;
                if include_raw {
                    txdetails.transaction = self.get_raw_tx(&txid)?;
                }

                Ok(txdetails)
            })
            .transpose()
    }

    fn get_last_index(&self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        let key = MapKey::LastIndex(keychain).as_map_key();
        self.get(key)?
            .map(|b| -> Result<_, Error> {
                let array: [u8; 4] = b
                    .as_ref()
                    .try_into()
                    .map_err(|_| Error::InvalidU32Bytes(b.to_vec()))?;
                let val = u32::from_be_bytes(array);
                Ok(val)
            })
            .transpose()
    }

    // inserts 0 if not present
    fn increment_last_index(&mut self, keychain: KeychainKind) -> Result<u32, Error> {
        let key = MapKey::LastIndex(keychain).as_map_key();
        self.update_and_fetch(key, |prev| {
            let new = match prev {
                Some(b) => {
                    let array: [u8; 4] = b.try_into().unwrap_or([0; 4]);
                    let val = u32::from_be_bytes(array);

                    val + 1
                }
                None => 0,
            };

            Some(new.to_be_bytes().to_vec())
        })?
        .map_or(Ok(0), |b| -> Result<_, Error> {
            let array: [u8; 4] = b
                .as_ref()
                .try_into()
                .map_err(|_| Error::InvalidU32Bytes(b.to_vec()))?;
            let val = u32::from_be_bytes(array);
            Ok(val)
        })
    }
}

impl BatchDatabase for Tree {
    type Batch = sled::Batch;

    fn begin_batch(&self) -> Self::Batch {
        sled::Batch::default()
    }

    fn commit_batch(&mut self, batch: Self::Batch) -> Result<(), Error> {
        Ok(self.apply_batch(batch)?)
    }
}

#[cfg(test)]
mod test {
    use std::sync::{Arc, Condvar, Mutex, Once};
    use std::time::{SystemTime, UNIX_EPOCH};

    use sled::{Db, Tree};

    static mut COUNT: usize = 0;

    lazy_static! {
        static ref DB: Arc<(Mutex<Option<Db>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        static ref INIT: Once = Once::new();
    }

    fn get_tree() -> Tree {
        unsafe {
            let cloned = DB.clone();
            let (mutex, cvar) = &*cloned;

            INIT.call_once(|| {
                let mut db = mutex.lock().unwrap();

                let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
                let mut dir = std::env::temp_dir();
                dir.push(format!("mbw_{}", time.as_nanos()));

                *db = Some(sled::open(dir).unwrap());
                cvar.notify_all();
            });

            let mut db = mutex.lock().unwrap();
            while !db.is_some() {
                db = cvar.wait(db).unwrap();
            }

            COUNT += 1;

            db.as_ref()
                .unwrap()
                .open_tree(format!("tree_{}", COUNT))
                .unwrap()
        }
    }

    #[test]
    fn test_script_pubkey() {
        crate::database::test::test_script_pubkey(get_tree());
    }

    #[test]
    fn test_batch_script_pubkey() {
        crate::database::test::test_batch_script_pubkey(get_tree());
    }

    #[test]
    fn test_iter_script_pubkey() {
        crate::database::test::test_iter_script_pubkey(get_tree());
    }

    #[test]
    fn test_del_script_pubkey() {
        crate::database::test::test_del_script_pubkey(get_tree());
    }

    #[test]
    fn test_utxo() {
        crate::database::test::test_utxo(get_tree());
    }

    #[test]
    fn test_raw_tx() {
        crate::database::test::test_raw_tx(get_tree());
    }

    #[test]
    fn test_tx() {
        crate::database::test::test_tx(get_tree());
    }

    #[test]
    fn test_last_index() {
        crate::database::test::test_last_index(get_tree());
    }
}
