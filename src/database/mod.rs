use bitcoin::hash_types::Txid;
use bitcoin::{OutPoint, Script, Transaction, TxOut};

use crate::error::Error;
use crate::types::*;

#[cfg(any(feature = "key-value-db", feature = "default"))]
pub mod keyvalue;
pub mod memory;

pub trait BatchOperations {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        script_type: ScriptType,
        child: u32,
    ) -> Result<(), Error>;
    fn set_utxo(&mut self, utxo: &UTXO) -> Result<(), Error>;
    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error>;
    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error>;
    fn set_last_index(&mut self, script_type: ScriptType, value: u32) -> Result<(), Error>;

    fn del_script_pubkey_from_path(
        &mut self,
        script_type: ScriptType,
        child: u32,
    ) -> Result<Option<Script>, Error>;
    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(ScriptType, u32)>, Error>;
    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error>;
    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error>;
    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, Error>;
    fn del_last_index(&mut self, script_type: ScriptType) -> Result<Option<u32>, Error>;
}

pub trait Database: BatchOperations {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        script_type: ScriptType,
        bytes: B,
    ) -> Result<(), Error>;

    fn iter_script_pubkeys(&self, script_type: Option<ScriptType>) -> Result<Vec<Script>, Error>;
    fn iter_utxos(&self) -> Result<Vec<UTXO>, Error>;
    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error>;
    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error>;

    fn get_script_pubkey_from_path(
        &self,
        script_type: ScriptType,
        child: u32,
    ) -> Result<Option<Script>, Error>;
    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ScriptType, u32)>, Error>;
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXO>, Error>;
    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error>;
    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error>;
    fn get_last_index(&self, script_type: ScriptType) -> Result<Option<u32>, Error>;

    // inserts 0 if not present
    fn increment_last_index(&mut self, script_type: ScriptType) -> Result<u32, Error>;
}

pub trait BatchDatabase: Database {
    type Batch: BatchOperations;

    fn begin_batch(&self) -> Self::Batch;
    fn commit_batch(&mut self, batch: Self::Batch) -> Result<(), Error>;
}

pub trait DatabaseUtils: Database {
    fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.get_path_from_script_pubkey(script)
            .map(|o| o.is_some())
    }

    fn get_raw_tx_or<F>(&self, txid: &Txid, f: F) -> Result<Option<Transaction>, Error>
    where
        F: FnOnce() -> Result<Option<Transaction>, Error>,
    {
        self.get_tx(txid, true)?
            .map(|t| t.transaction)
            .flatten()
            .map_or_else(f, |t| Ok(Some(t)))
    }

    fn get_previous_output(&self, outpoint: &OutPoint) -> Result<Option<TxOut>, Error> {
        self.get_raw_tx(&outpoint.txid)?
            .and_then(|previous_tx| {
                if outpoint.vout as usize >= previous_tx.output.len() {
                    Some(Err(Error::InvalidOutpoint(outpoint.clone())))
                } else {
                    Some(Ok(previous_tx.output[outpoint.vout as usize].clone()))
                }
            })
            .transpose()
    }
}

impl<T: Database> DatabaseUtils for T {}

#[cfg(test)]
pub mod test {
    use std::str::FromStr;

    use bitcoin::consensus::encode::deserialize;
    use bitcoin::hashes::hex::*;
    use bitcoin::*;

    use super::*;

    pub fn test_script_pubkey<D: Database>(mut tree: D) {
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

    pub fn test_batch_script_pubkey<D: BatchDatabase>(mut tree: D) {
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
            Some((script_type, path.clone()))
        );
    }

    pub fn test_iter_script_pubkey<D: Database>(mut tree: D) {
        let script = Script::from(
            Vec::<u8>::from_hex("76a91402306a7c23f3e8010de41e9e591348bb83f11daa88ac").unwrap(),
        );
        let path = 42;
        let script_type = ScriptType::External;

        tree.set_script_pubkey(&script, script_type, path).unwrap();

        assert_eq!(tree.iter_script_pubkeys(None).unwrap().len(), 1);
    }

    pub fn test_del_script_pubkey<D: Database>(mut tree: D) {
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

    pub fn test_utxo<D: Database>(mut tree: D) {
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
        let utxo = UTXO {
            txout,
            outpoint,
            is_internal: false,
        };

        tree.set_utxo(&utxo).unwrap();

        assert_eq!(tree.get_utxo(&outpoint).unwrap(), Some(utxo));
    }

    pub fn test_raw_tx<D: Database>(mut tree: D) {
        let hex_tx = Vec::<u8>::from_hex("0100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000").unwrap();
        let tx: Transaction = deserialize(&hex_tx).unwrap();

        tree.set_raw_tx(&tx).unwrap();

        let txid = tx.txid();

        assert_eq!(tree.get_raw_tx(&txid).unwrap(), Some(tx));
    }

    pub fn test_tx<D: Database>(mut tree: D) {
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

    pub fn test_last_index<D: Database>(mut tree: D) {
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
