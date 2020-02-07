use bitcoin::hash_types::Txid;
use bitcoin::util::bip32::{ChildNumber, DerivationPath};
use bitcoin::{OutPoint, Script, Transaction};

use crate::error::Error;
use crate::types::*;

#[cfg(any(feature = "key-value-db", feature = "default"))]
pub mod keyvalue;

pub trait BatchOperations {
    fn set_script_pubkey<P: AsRef<[ChildNumber]>>(
        &mut self,
        script: &Script,
        script_type: ScriptType,
        path: &P,
    ) -> Result<(), Error>;
    fn set_utxo(&mut self, utxo: &UTXO) -> Result<(), Error>;
    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error>;
    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error>;
    fn set_last_index(&mut self, script_type: ScriptType, value: u32) -> Result<(), Error>;

    fn del_script_pubkey_from_path<P: AsRef<[ChildNumber]>>(
        &mut self,
        script_type: ScriptType,
        path: &P,
    ) -> Result<Option<Script>, Error>;
    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(ScriptType, DerivationPath)>, Error>;
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
    fn iter_script_pubkeys(&self, script_type: Option<ScriptType>) -> Result<Vec<Script>, Error>;
    fn iter_utxos(&self) -> Result<Vec<UTXO>, Error>;
    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error>;
    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error>;

    fn get_script_pubkey_from_path<P: AsRef<[ChildNumber]>>(
        &self,
        script_type: ScriptType,
        path: &P,
    ) -> Result<Option<Script>, Error>;
    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ScriptType, DerivationPath)>, Error>;
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
