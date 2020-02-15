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
// outpoint             u<outpoint> -> txout
// rawtx                r<txid> -> tx
// transactions         t<txid> -> tx details
// deriv indexes        c{i,e} -> u32
// descriptor checksum  d{i,e} -> vec<u8>

enum SledKey<'a> {
    Path((Option<ScriptType>, Option<&'a DerivationPath>)),
    Script(Option<&'a Script>),
    UTXO(Option<&'a OutPoint>),
    RawTx(Option<&'a Txid>),
    Transaction(Option<&'a Txid>),
    LastIndex(ScriptType),
    DescriptorChecksum(ScriptType),
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
            SledKey::DescriptorChecksum(st) => [b"d", st.as_ref()].concat(),
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
        fn set_script_pubkey<P: AsRef<[ChildNumber]>>(&mut self, script: &Script, script_type: ScriptType, path: &P) -> Result<(), Error> {
            let deriv_path = DerivationPath::from(path.as_ref());
            let key = SledKey::Path((Some(script_type), Some(&deriv_path))).as_sled_key();
            self.insert(key, serialize(script))$($after_insert)*;

            let key = SledKey::Script(Some(script)).as_sled_key();
            let value = json!({
                "t": script_type,
                "p": deriv_path,
            });
            self.insert(key, serde_json::to_vec(&value)?)$($after_insert)*;

            Ok(())
        }

        fn set_utxo(&mut self, utxo: &UTXO) -> Result<(), Error> {
            let key = SledKey::UTXO(Some(&utxo.outpoint)).as_sled_key();
            let value = serialize(&utxo.txout);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
            let key = SledKey::RawTx(Some(&transaction.txid())).as_sled_key();
            let value = serialize(transaction);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error> {
            let key = SledKey::Transaction(Some(&transaction.txid)).as_sled_key();

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

        fn set_last_index(&mut self, script_type: ScriptType, value: u32) -> Result<(), Error> {
            let key = SledKey::LastIndex(script_type).as_sled_key();
            self.insert(key, &value.to_be_bytes())$($after_insert)*;

            Ok(())
        }

        fn del_script_pubkey_from_path<P: AsRef<[ChildNumber]>>(&mut self, script_type: ScriptType, path: &P) -> Result<Option<Script>, Error> {
            let deriv_path = DerivationPath::from(path.as_ref());
            let key = SledKey::Path((Some(script_type), Some(&deriv_path))).as_sled_key();
            let res = self.remove(key);
            let res = $process_delete!(res);

            Ok(res.map_or(Ok(None), |x| Some(deserialize(&x)).transpose())?)
        }

        fn del_path_from_script_pubkey(&mut self, script: &Script) -> Result<Option<(ScriptType, DerivationPath)>, Error> {
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
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        script_type: ScriptType,
        bytes: B,
    ) -> Result<(), Error> {
        let key = SledKey::DescriptorChecksum(script_type).as_sled_key();

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

    fn iter_script_pubkeys(&self, script_type: Option<ScriptType>) -> Result<Vec<Script>, Error> {
        let key = SledKey::Path((script_type, None)).as_sled_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (_, v) = x?;
                Ok(deserialize(&v)?)
            })
            .collect()
    }

    fn iter_utxos(&self) -> Result<Vec<UTXO>, Error> {
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

    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error> {
        let key = SledKey::RawTx(None).as_sled_key();
        self.scan_prefix(key)
            .map(|x| -> Result<_, Error> {
                let (_, v) = x?;
                Ok(deserialize(&v)?)
            })
            .collect()
    }

    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        let key = SledKey::Transaction(None).as_sled_key();
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
        path: &P,
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
    fn increment_last_index(&mut self, script_type: ScriptType) -> Result<u32, Error> {
        let key = SledKey::LastIndex(script_type).as_sled_key();
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
    use std::str::FromStr;
    use std::sync::{Arc, Condvar, Mutex, Once};
    use std::time::{SystemTime, UNIX_EPOCH};

    use sled::{Db, Tree};

    use bitcoin::consensus::encode::deserialize;
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
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = DerivationPath::from_str("m/0/1/2/3").unwrap();
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, &path).unwrap();

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
            .set_script_pubkey(&script, script_type, &path)
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

        tree.set_script_pubkey(&script, script_type, &path).unwrap();

        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);
    }

    #[test]
    fn test_del_script_pubkey() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = DerivationPath::from_str("m/0/1/2/3").unwrap();
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, &path).unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);

        tree.del_script_pubkey_from_path(script_type, &path)
            .unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 0);
    }

    #[test]
    fn test_utxo() {
        let mut tree = get_tree();

        let outpoint = OutPoint::from_str(
            "5df6e0e2761359d30a8275058e299fcc0381534545f55cf43e41983f5d4c9456:0",
        )
        .unwrap();
        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let txout = TxOut {
            value: 133742,
            script_pubkey: script,
        };
        let utxo = UTXO { txout, outpoint };

        tree.set_utxo(&utxo).unwrap();

        assert_eq!(tree.get_utxo(&outpoint).unwrap(), Some(utxo));
    }

    #[test]
    fn test_raw_tx() {
        let mut tree = get_tree();

        let hex_tx = Vec::<u8>::from_hex("0100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();
        let tx: Transaction = deserialize(&hex_tx).unwrap();

        tree.set_raw_tx(&tx).unwrap();

        let txid = tx.txid();

        assert_eq!(tree.get_raw_tx(&txid).unwrap(), Some(tx));
    }

    #[test]
    fn test_tx() {
        let mut tree = get_tree();

        let hex_tx = Vec::<u8>::from_hex("0100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();
        let tx: Transaction = deserialize(&hex_tx).unwrap();
        let txid = tx.txid();
        let mut tx_details = TransactionDetails {
            transaction: Some(tx),
            txid,
            timestamp: 123456,
            received: 1337,
            sent: 420420,
            height: Some(1000),
        };

        tree.set_tx(&tx_details).unwrap();

        // get with raw tx too
        assert_eq!(
            tree.get_tx(&tx_details.txid, true).unwrap(),
            Some(tx_details.clone())
        );
        // get only raw_tx
        assert_eq!(
            tree.get_raw_tx(&tx_details.txid).unwrap(),
            tx_details.transaction
        );

        // now get without raw_tx
        tx_details.transaction = None;
        assert_eq!(
            tree.get_tx(&tx_details.txid, false).unwrap(),
            Some(tx_details)
        );
    }

    #[test]
    fn test_last_index() {
        let mut tree = get_tree();

        tree.set_last_index(ScriptType::External, 1337).unwrap();

        assert_eq!(
            tree.get_last_index(ScriptType::External).unwrap(),
            Some(1337)
        );
        assert_eq!(tree.get_last_index(ScriptType::Internal).unwrap(), None);

        let res = tree.increment_last_index(ScriptType::External).unwrap();
        assert_eq!(res, 1337);
        let res = tree.increment_last_index(ScriptType::Internal).unwrap();
        assert_eq!(res, 0);

        assert_eq!(
            tree.get_last_index(ScriptType::External).unwrap(),
            Some(1338)
        );
        assert_eq!(tree.get_last_index(ScriptType::Internal).unwrap(), Some(1));
    }

    // TODO: more tests...
}
