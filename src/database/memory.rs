use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included};

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hash_types::Txid;
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

pub(crate) enum MapKey<'a> {
    Path((Option<ScriptType>, Option<u32>)),
    Script(Option<&'a Script>),
    UTXO(Option<&'a OutPoint>),
    RawTx(Option<&'a Txid>),
    Transaction(Option<&'a Txid>),
    LastIndex(ScriptType),
    DescriptorChecksum(ScriptType),
}

impl MapKey<'_> {
    pub fn as_prefix(&self) -> Vec<u8> {
        match self {
            MapKey::Path((st, _)) => {
                let mut v = b"p".to_vec();
                if let Some(st) = st {
                    v.push(st.as_byte());
                }
                v
            }
            MapKey::Script(_) => b"s".to_vec(),
            MapKey::UTXO(_) => b"u".to_vec(),
            MapKey::RawTx(_) => b"r".to_vec(),
            MapKey::Transaction(_) => b"t".to_vec(),
            MapKey::LastIndex(st) => [b"c", st.as_ref()].concat(),
            MapKey::DescriptorChecksum(st) => [b"d", st.as_ref()].concat(),
        }
    }

    fn serialize_content(&self) -> Vec<u8> {
        match self {
            MapKey::Path((_, Some(child))) => u32::from(*child).to_be_bytes().to_vec(),
            MapKey::Script(Some(s)) => serialize(*s),
            MapKey::UTXO(Some(s)) => serialize(*s),
            MapKey::RawTx(Some(s)) => serialize(*s),
            MapKey::Transaction(Some(s)) => serialize(*s),
            _ => vec![],
        }
    }

    pub fn as_map_key(&self) -> Vec<u8> {
        let mut v = self.as_prefix();
        v.extend_from_slice(&self.serialize_content());

        v
    }
}

fn after(key: &Vec<u8>) -> Vec<u8> {
    let mut key = key.clone();
    let len = key.len();
    if len > 0 {
        // TODO i guess it could break if the value is 0xFF, but it's fine for now
        key[len - 1] += 1;
    }

    key
}

#[derive(Debug)]
pub struct MemoryDatabase {
    map: BTreeMap<Vec<u8>, Box<dyn std::any::Any>>,
    deleted_keys: Vec<Vec<u8>>,
}

impl MemoryDatabase {
    pub fn new() -> Self {
        MemoryDatabase {
            map: BTreeMap::new(),
            deleted_keys: Vec::new(),
        }
    }
}

impl BatchOperations for MemoryDatabase {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        script_type: ScriptType,
        path: u32,
    ) -> Result<(), Error> {
        let key = MapKey::Path((Some(script_type), Some(path))).as_map_key();
        self.map.insert(key, Box::new(script.clone()));

        let key = MapKey::Script(Some(script)).as_map_key();
        let value = json!({
            "t": script_type,
            "p": path,
        });
        self.map.insert(key, Box::new(value));

        Ok(())
    }

    fn set_utxo(&mut self, utxo: &UTXO) -> Result<(), Error> {
        let key = MapKey::UTXO(Some(&utxo.outpoint)).as_map_key();
        self.map.insert(key, Box::new(utxo.txout.clone()));

        Ok(())
    }
    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
        let key = MapKey::RawTx(Some(&transaction.txid())).as_map_key();
        self.map.insert(key, Box::new(transaction.clone()));

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

        self.map.insert(key, Box::new(transaction));

        Ok(())
    }
    fn set_last_index(&mut self, script_type: ScriptType, value: u32) -> Result<(), Error> {
        let key = MapKey::LastIndex(script_type).as_map_key();
        self.map.insert(key, Box::new(value));

        Ok(())
    }

    fn del_script_pubkey_from_path(
        &mut self,
        script_type: ScriptType,
        path: u32,
    ) -> Result<Option<Script>, Error> {
        let key = MapKey::Path((Some(script_type), Some(path))).as_map_key();
        let res = self.map.remove(&key);
        self.deleted_keys.push(key);

        Ok(res.map(|x| x.downcast_ref().cloned().unwrap()))
    }
    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(ScriptType, u32)>, Error> {
        let key = MapKey::Script(Some(script)).as_map_key();
        let res = self.map.remove(&key);
        self.deleted_keys.push(key);

        match res {
            None => Ok(None),
            Some(b) => {
                let mut val: serde_json::Value = b.downcast_ref().cloned().unwrap();
                let st = serde_json::from_value(val["t"].take())?;
                let path = serde_json::from_value(val["p"].take())?;

                Ok(Some((st, path)))
            }
        }
    }
    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error> {
        let key = MapKey::UTXO(Some(outpoint)).as_map_key();
        let res = self.map.remove(&key);
        self.deleted_keys.push(key);

        match res {
            None => Ok(None),
            Some(b) => {
                let txout = b.downcast_ref().cloned().unwrap();
                Ok(Some(UTXO {
                    outpoint: outpoint.clone(),
                    txout,
                }))
            }
        }
    }
    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let key = MapKey::RawTx(Some(txid)).as_map_key();
        let res = self.map.remove(&key);
        self.deleted_keys.push(key);

        Ok(res.map(|x| x.downcast_ref().cloned().unwrap()))
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
        let res = self.map.remove(&key);
        self.deleted_keys.push(key);

        match res {
            None => Ok(None),
            Some(b) => {
                let mut val: TransactionDetails = b.downcast_ref().cloned().unwrap();
                val.transaction = raw_tx;

                Ok(Some(val))
            }
        }
    }
    fn del_last_index(&mut self, script_type: ScriptType) -> Result<Option<u32>, Error> {
        let key = MapKey::LastIndex(script_type).as_map_key();
        let res = self.map.remove(&key);
        self.deleted_keys.push(key);

        match res {
            None => Ok(None),
            Some(b) => Ok(Some(*b.downcast_ref().unwrap())),
        }
    }
}

impl Database for MemoryDatabase {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        script_type: ScriptType,
        bytes: B,
    ) -> Result<(), Error> {
        let key = MapKey::DescriptorChecksum(script_type).as_map_key();

        let prev = self
            .map
            .get(&key)
            .map(|x| x.downcast_ref::<Vec<u8>>().unwrap());
        if let Some(val) = prev {
            if val == &bytes.as_ref().to_vec() {
                Ok(())
            } else {
                Err(Error::ChecksumMismatch)
            }
        } else {
            self.map.insert(key, Box::new(bytes.as_ref().to_vec()));
            Ok(())
        }
    }

    fn iter_script_pubkeys(&self, script_type: Option<ScriptType>) -> Result<Vec<Script>, Error> {
        let key = MapKey::Path((script_type, None)).as_map_key();
        self.map
            .range::<Vec<u8>, _>((Included(&key), Excluded(&after(&key))))
            .map(|(_, v)| Ok(v.downcast_ref().cloned().unwrap()))
            .collect()
    }

    fn iter_utxos(&self) -> Result<Vec<UTXO>, Error> {
        let key = MapKey::UTXO(None).as_map_key();
        self.map
            .range::<Vec<u8>, _>((Included(&key), Excluded(&after(&key))))
            .map(|(k, v)| {
                let outpoint = deserialize(&k[1..]).unwrap();
                let txout = v.downcast_ref().cloned().unwrap();
                Ok(UTXO { outpoint, txout })
            })
            .collect()
    }

    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error> {
        let key = MapKey::RawTx(None).as_map_key();
        self.map
            .range::<Vec<u8>, _>((Included(&key), Excluded(&after(&key))))
            .map(|(_, v)| Ok(v.downcast_ref().cloned().unwrap()))
            .collect()
    }

    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        let key = MapKey::Transaction(None).as_map_key();
        self.map
            .range::<Vec<u8>, _>((Included(&key), Excluded(&after(&key))))
            .map(|(k, v)| {
                let mut txdetails: TransactionDetails = v.downcast_ref().cloned().unwrap();
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
        script_type: ScriptType,
        path: u32,
    ) -> Result<Option<Script>, Error> {
        let key = MapKey::Path((Some(script_type), Some(path))).as_map_key();
        Ok(self
            .map
            .get(&key)
            .map(|b| b.downcast_ref().cloned().unwrap()))
    }

    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ScriptType, u32)>, Error> {
        let key = MapKey::Script(Some(script)).as_map_key();
        Ok(self.map.get(&key).map(|b| {
            let mut val: serde_json::Value = b.downcast_ref().cloned().unwrap();
            let st = serde_json::from_value(val["t"].take()).unwrap();
            let path = serde_json::from_value(val["p"].take()).unwrap();

            (st, path)
        }))
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error> {
        let key = MapKey::UTXO(Some(outpoint)).as_map_key();
        Ok(self.map.get(&key).map(|b| {
            let txout = b.downcast_ref().cloned().unwrap();
            UTXO {
                outpoint: outpoint.clone(),
                txout,
            }
        }))
    }

    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let key = MapKey::RawTx(Some(txid)).as_map_key();
        Ok(self
            .map
            .get(&key)
            .map(|b| b.downcast_ref().cloned().unwrap()))
    }

    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        let key = MapKey::Transaction(Some(txid)).as_map_key();
        Ok(self.map.get(&key).map(|b| {
            let mut txdetails: TransactionDetails = b.downcast_ref().cloned().unwrap();
            if include_raw {
                txdetails.transaction = self.get_raw_tx(&txid).unwrap();
            }

            txdetails
        }))
    }

    fn get_last_index(&self, script_type: ScriptType) -> Result<Option<u32>, Error> {
        let key = MapKey::LastIndex(script_type).as_map_key();
        Ok(self.map.get(&key).map(|b| *b.downcast_ref().unwrap()))
    }

    // inserts 0 if not present
    fn increment_last_index(&mut self, script_type: ScriptType) -> Result<u32, Error> {
        let key = MapKey::LastIndex(script_type).as_map_key();
        let value = self
            .map
            .entry(key.clone())
            .and_modify(|x| *x.downcast_mut::<u32>().unwrap() += 1)
            .or_insert(Box::<u32>::new(0))
            .downcast_mut()
            .unwrap();

        Ok(*value)
    }
}

impl BatchDatabase for MemoryDatabase {
    type Batch = Self;

    fn begin_batch(&self) -> Self::Batch {
        MemoryDatabase::new()
    }

    fn commit_batch(&mut self, mut batch: Self::Batch) -> Result<(), Error> {
        for key in batch.deleted_keys {
            self.map.remove(&key);
        }

        Ok(self.map.append(&mut batch.map))
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::consensus::encode::deserialize;
    use bitcoin::hashes::hex::*;
    use bitcoin::*;

    use super::*;
    use crate::database::*;

    fn get_tree() -> MemoryDatabase {
        MemoryDatabase::new()
    }

    #[test]
    fn test_script_pubkey() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = 42;
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, path).unwrap();

        assert_eq!(
            tree.get_script_pubkey_from_path(script_type, path).unwrap(),
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
        let path = 42;
        let script_type = ScriptType::External;

        batch.set_script_pubkey(&script, script_type, path).unwrap();

        assert_eq!(
            tree.get_script_pubkey_from_path(script_type, path).unwrap(),
            None
        );
        assert_eq!(tree.get_path_from_script_pubkey(&script).unwrap(), None);

        tree.commit_batch(batch).unwrap();

        assert_eq!(
            tree.get_script_pubkey_from_path(script_type, path).unwrap(),
            Some(script.clone())
        );
        assert_eq!(
            tree.get_path_from_script_pubkey(&script).unwrap(),
            Some((script_type, path))
        );
    }

    #[test]
    fn test_iter_script_pubkey() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = 42;
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, path).unwrap();

        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);
    }

    #[test]
    fn test_del_script_pubkey() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = 42;
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, path).unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);

        tree.del_script_pubkey_from_path(script_type, path).unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 0);
    }

    #[test]
    fn test_del_script_pubkey_batch() {
        let mut tree = get_tree();

        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = 42;
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, path).unwrap();
        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);

        let mut batch = tree.begin_batch();
        batch
            .del_script_pubkey_from_path(script_type, path)
            .unwrap();

        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);

        tree.commit_batch(batch).unwrap();
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
        assert_eq!(res, 1338);
        let res = tree.increment_last_index(ScriptType::Internal).unwrap();
        assert_eq!(res, 0);

        assert_eq!(
            tree.get_last_index(ScriptType::External).unwrap(),
            Some(1338)
        );
        assert_eq!(tree.get_last_index(ScriptType::Internal).unwrap(), Some(0));
    }

    // TODO: more tests...
}
