use std::convert::AsRef;

use bitcoin::blockdata::transaction::{OutPoint, Transaction, TxOut};
use bitcoin::hash_types::Txid;

use serde::{Deserialize, Serialize};

// TODO serde flatten?
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptType {
    External = 0,
    Internal = 1,
}

impl ScriptType {
    pub fn as_byte(&self) -> u8 {
        match self {
            ScriptType::External => 'e' as u8,
            ScriptType::Internal => 'i' as u8,
        }
    }
}

impl AsRef<[u8]> for ScriptType {
    fn as_ref(&self) -> &[u8] {
        match self {
            ScriptType::External => b"e",
            ScriptType::Internal => b"i",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UTXO {
    pub outpoint: OutPoint,
    pub txout: TxOut,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct TransactionDetails {
    pub transaction: Option<Transaction>,
    pub txid: Txid,
    pub timestamp: u64,
    pub received: u64,
    pub sent: u64,
    pub height: Option<u32>,
}
