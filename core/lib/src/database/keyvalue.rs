use std::convert::{From, TryInto};

use sled::{Batch, Tree};

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hash_types::Txid;
use bitcoin::util::bip32::{ChildNumber, DerivationPath};
use bitcoin::{OutPoint, Script, Transaction};

use crate::database::{BatchDatabase, BatchOperations, Database};
use crate::error::Error;
use crate::types::*;

// path -> script       p{i,e}<path> -> script
// script -> path       s<script> -> {i,e}<path>
// outpoint ->          u -> utxo
// txhash ->            r -> tx
// txhash ->            t -> tx details
// change indexes       c{i,e} -> u32

enum SledKey<'a> {
    Path((Option<ScriptType>, Option<&'a DerivationPath>)),
    Script(Option<&'a Script>),
    UTXO(Option<&'a OutPoint>),
    RawTx(Option<&'a Txid>),
    Transaction(Option<&'a Txid>),
    LastIndex(ScriptType),
}

impl SledKey<'_> {
    pub fn as_prefix(&self) -> Vec<u8> {
        match self {
            SledKey::Path((st, _)) => {
                let mut v = b"p".to_vec();
                if let Some(st) = st {
                    v.push(st.as_byte());
                }
                v
            }
            SledKey::Script(_) => b"s".to_vec(),
            SledKey::UTXO(_) => b"u".to_vec(),
            SledKey::RawTx(_) => b"r".to_vec(),
            SledKey::Transaction(_) => b"t".to_vec(),
            SledKey::LastIndex(st) => [b"c", st.as_ref()].concat(),
        }
    }

    fn serialize_content(&self) -> Vec<u8> {
        match self {
            SledKey::Path((_, Some(path))) => {
                let mut res = vec![];
                for val in *path {
                    res.extend(&u32::from(*val).to_be_bytes());
                }
                res
            }
            SledKey::Script(Some(s)) => serialize(*s),
            SledKey::UTXO(Some(s)) => serialize(*s),
            SledKey::RawTx(Some(s)) => serialize(*s),
            SledKey::Transaction(Some(s)) => serialize(*s),
            _ => vec![],
        }
    }

    pub fn as_sled_key(&self) -> Vec<u8> {
        let mut v = self.as_prefix();
        v.extend_from_slice(&self.serialize_content());

        v
    }
}

macro_rules! impl_batch_operations {
    ( { $($after_insert:tt)* }, $process_delete:ident ) => {
        fn set_script_pubkey<P: AsRef<[ChildNumber]>>(&mut self, script: Script, script_type: ScriptType, path: P) -> Result<(), Error> {
            let deriv_path = DerivationPath::from(path.as_ref());
            let key = SledKey::Path((Some(script_type), Some(&deriv_path))).as_sled_key();
            self.insert(key, serialize(&script))$($after_insert)*;

            let key = SledKey::Script(Some(&script)).as_sled_key();
            let value = json!({
                "t": script_type,
                "p": deriv_path,
            });
            self.insert(key, serde_json::to_vec(&value)?)$($after_insert)*;

            Ok(())
        }

        fn set_utxo(&mut self, utxo: UTXO) -> Result<(), Error> {
            let key = SledKey::UTXO(Some(&utxo.outpoint)).as_sled_key();
            let value = serialize(&utxo.txout);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_raw_tx(&mut self, transaction: Transaction) -> Result<(), Error> {
            let key = SledKey::RawTx(Some(&transaction.txid())).as_sled_key();
            let value = serialize(&transaction);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_tx(&mut self, mut transaction: TransactionDetails) -> Result<(), Error> {
            let key = SledKey::Transaction(Some(&transaction.txid)).as_sled_key();
            // remove the raw tx
            transaction.transaction = None;
            let value = serde_json::to_vec(&transaction)?;
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_last_index(&mut self, script_type: ScriptType, value: u32) -> Result<(), Error> {
            let key = SledKey::LastIndex(script_type).as_sled_key();
            self.insert(key, &value.to_be_bytes())$($after_insert)*;

            Ok(())
        }

        fn del_script_pubkey_from_path<P: AsRef<[ChildNumber]>>(&mut self, script_type: ScriptType, path: P) -> Result<Option<Script>, Error> {
            // TODO: delete the other too?
            let deriv_path = DerivationPath::from(path.as_ref());
            let key = SledKey::Path((Some(script_type), Some(&deriv_path))).as_sled_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            Ok(res.map_or(Ok(None), |x| Some(deserialize(&x)).transpose())?)
        }

        fn del_path_from_script_pubkey(&mut self, script: &Script) -> Result<Option<(ScriptType, DerivationPath)>, Error> {
            // TODO: delete the other too?
            let key = SledKey::Script(Some(script)).as_sled_key();
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
            let key = SledKey::UTXO(Some(outpoint)).as_sled_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            match res {
                None => Ok(None),
                Some(b) => {
                    let txout = deserialize(&b)?;
                    Ok(Some(UTXO { outpoint: outpoint.clone(), txout }))
                }
            }
        }

        fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
            let key = SledKey::RawTx(Some(txid)).as_sled_key();
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

            let key = SledKey::Transaction(Some(txid)).as_sled_key();
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

        fn del_last_index(&mut self, script_type: ScriptType) -> Result<Option<u32>, Error> {
            let key = SledKey::LastIndex(script_type).as_sled_key();
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
    fn iter_script_pubkeys(&self, script_type: Option<ScriptType>) -> Vec<Result<Script, Error>> {
        let key = SledKey::Path((script_type, None)).as_sled_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (_, v) = x?;
                Ok(deserialize(&v)?)
            })
            .collect()
    }

    fn iter_utxos(&self) -> Vec<Result<UTXO, Error>> {
        let key = SledKey::UTXO(None).as_sled_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (k, v) = x?;
                let outpoint = deserialize(&k[1..])?;
                let txout = deserialize(&v)?;
                Ok(UTXO { outpoint, txout })
            })
            .collect()
    }

    fn iter_raw_txs(&self) -> Vec<Result<Transaction, Error>> {
        let key = SledKey::RawTx(None).as_sled_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (_, v) = x?;
                Ok(deserialize(&v)?)
            })
            .collect()
    }

    fn iter_txs(&self, include_raw: bool) -> Vec<Result<TransactionDetails, Error>> {
        let key = SledKey::RawTx(None).as_sled_key();
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

    fn get_script_pubkey_from_path<P: AsRef<[ChildNumber]>>(
        &self,
        script_type: ScriptType,
        path: P,
    ) -> Result<Option<Script>, Error> {
        let deriv_path = DerivationPath::from(path.as_ref());
        let key = SledKey::Path((Some(script_type), Some(&deriv_path))).as_sled_key();
        Ok(self.get(key)?.map(|b| deserialize(&b)).transpose()?)
    }

    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ScriptType, DerivationPath)>, Error> {
        let key = SledKey::Script(Some(script)).as_sled_key();
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
        let key = SledKey::UTXO(Some(outpoint)).as_sled_key();
        self.get(key)?
            .map(|b| -> Result<_, Error> {
                let txout = deserialize(&b)?;
                Ok(UTXO {
                    outpoint: outpoint.clone(),
                    txout,
                })
            })
            .transpose()
    }

    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let key = SledKey::RawTx(Some(txid)).as_sled_key();
        Ok(self.get(key)?.map(|b| deserialize(&b)).transpose()?)
    }

    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        let key = SledKey::Transaction(Some(txid)).as_sled_key();
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

    fn get_last_index(&self, script_type: ScriptType) -> Result<Option<u32>, Error> {
        let key = SledKey::LastIndex(script_type).as_sled_key();
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
    fn increment_last_index(&mut self, script_type: ScriptType) -> Result<Option<u32>, Error> {
        let key = SledKey::LastIndex(script_type).as_sled_key();
        self.fetch_and_update(key, |prev| {
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
    use std::str::FromStr;
    use std::sync::{Arc, Condvar, Mutex, Once};
    use std::time::Instant;

    use sled::{Db, Tree};

    use bitcoin::hashes::hex::*;
    use bitcoin::*;

    use crate::database::*;

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

                let start = Instant::now();
                let mut dir = std::env::temp_dir();
                dir.push(format!("{:?}", start));

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
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = DerivationPath::from_str("m/0/1/2/3").unwrap();
        let script_type = ScriptType::External;

        tree.set_script_pubkey(script.clone(), script_type, path.clone())
            .unwrap();

        assert_eq!(
            tree.get_script_pubkey_from_path(script_type, &path)
                .unwrap(),
            Some(script.clone())
        );
        assert_eq!(
            tree.get_path_from_script_pubkey(&script).unwrap(),
            Some((script_type, path.clone()))
        );
    }

    #[test]
    fn test_batch_script_pubkey() {
        let mut tree = get_tree();
        let mut batch = tree.begin_batch();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = DerivationPath::from_str("m/0/1/2/3").unwrap();
        let script_type = ScriptType::External;

        batch
            .set_script_pubkey(script.clone(), script_type, path.clone())
            .unwrap();

        assert_eq!(
            tree.get_script_pubkey_from_path(script_type, &path)
                .unwrap(),
            None
        );
        assert_eq!(tree.get_path_from_script_pubkey(&script).unwrap(), None);

        tree.commit_batch(batch).unwrap();

        assert_eq!(
            tree.get_script_pubkey_from_path(script_type, &path)
                .unwrap(),
            Some(script.clone())
        );
        assert_eq!(
            tree.get_path_from_script_pubkey(&script).unwrap(),
            Some((script_type, path.clone()))
        );
    }

    #[test]
    fn test_iter_script_pubkey() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = DerivationPath::from_str("m/0/1/2/3").unwrap();
        let script_type = ScriptType::External;

        tree.set_script_pubkey(script.clone(), script_type, path.clone())
            .unwrap();

        assert_eq!(tree.iter_script_pubkeys(None).len(), 1);
    }

    #[test]
    fn test_del_script_pubkey() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = DerivationPath::from_str("m/0/1/2/3").unwrap();
        let script_type = ScriptType::External;

        tree.set_script_pubkey(script.clone(), script_type, path.clone())
            .unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).len(), 1);

        tree.del_script_pubkey_from_path(script_type, &path)
            .unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).len(), 0);
    }

    // TODO: more tests...
}
