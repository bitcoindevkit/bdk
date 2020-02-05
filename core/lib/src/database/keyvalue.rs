use std::borrow::Borrow;
use std::convert::{From, TryInto};

use sled::{Batch, Db, IVec, Tree};

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hash_types::Txid;
use bitcoin::util::bip32::{ChildNumber, DerivationPath};
use bitcoin::{OutPoint, Script, Transaction, TxOut};

use crate::database::{BatchDatabase, BatchOperations, Database};
use crate::error::Error;
use crate::types::*;

// TODO: rename mod to Sled?

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

        fn set_utxo(&mut self, outpoint: OutPoint, txout: TxOut) -> Result<(), Error> {
            let key = SledKey::UTXO(Some(&outpoint)).as_sled_key();
            let value = serialize(&txout);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_raw_tx(&mut self, transaction: Transaction) -> Result<(), Error> {
            let key = SledKey::RawTx(Some(&transaction.txid())).as_sled_key();
            let value = serialize(&transaction);
            self.insert(key, value)$($after_insert)*;

            Ok(())
        }

        fn set_tx(&mut self, transaction: TransactionDetails) -> Result<(), Error> {
            let key = SledKey::Transaction(Some(&transaction.txid)).as_sled_key();
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
                    if b.len() < 4 {
                        return Err(Error::Generic("Can't create a u32".to_string()));
                    }

                    let bytes: &[u8] = b.borrow();
                    let val = u32::from_be_bytes(bytes.try_into().unwrap());
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
impl BatchOperations for Batch {
    impl_batch_operations!({}, process_delete_batch);
}

impl Database for Tree {
    fn iter_script_pubkeys(&self, script_type: Option<ScriptType>) -> Vec<Result<Script, Error>> {
        let key = SledKey::Path((script_type, None)).as_sled_key();
        self.scan_prefix(key)
            .map(|x| {
                x.map_err(Error::Sled)
                    .and_then(|(_, v)| deserialize(&v).map_err(Error::Encode))
            })
            .collect()
    }

    fn iter_utxos(&self) -> Vec<Result<(OutPoint, TxOut), Error>> {
        vec![]
    }
    fn iter_raw_txs(&self) -> Vec<Result<Transaction, Error>> {
        vec![]
    }
    fn iter_txs(&self, include_raw: bool) -> Vec<Result<TransactionDetails, Error>> {
        vec![]
    }

    fn get_script_pubkey_from_path<P: AsRef<[ChildNumber]>>(
        &self,
        script_type: ScriptType,
        path: P,
    ) -> Result<Option<Script>, Error> {
        Err(Error::Generic("".to_string()))
    }
    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ScriptType, Script)>, Error> {
        Err(Error::Generic("".to_string()))
    }
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error> {
        Err(Error::Generic("".to_string()))
    }
    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Err(Error::Generic("".to_string()))
    }
    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        Err(Error::Generic("".to_string()))
    }
    fn get_last_index(&self, script_type: ScriptType) -> Result<Option<u32>, Error> {
        Err(Error::Generic("".to_string()))
    }

    // inserts 0 if not present
    fn increment_last_index(&mut self, script_type: ScriptType) -> Result<u32, Error> {
        Err(Error::Generic("".to_string()))
    }
}
