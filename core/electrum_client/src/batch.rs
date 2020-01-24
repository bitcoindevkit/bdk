use bitcoin::hashes::hex::ToHex;
use bitcoin::{Script, Txid};

use types::{Param, ToElectrumScriptHash};

pub struct Batch {
    calls: Vec<(String, Vec<Param>)>,
}

impl Batch {
    pub fn script_list_unspent(&mut self, script: &Script) {
        let params = vec![Param::String(script.to_electrum_scripthash().to_hex())];
        self.calls
            .push((String::from("blockchain.scripthash.listunspent"), params));
    }

    pub fn script_get_history(&mut self, script: &Script) {
        let params = vec![Param::String(script.to_electrum_scripthash().to_hex())];
        self.calls
            .push((String::from("blockchain.scripthash.get_history"), params));
    }

    pub fn script_get_balance(&mut self, script: &Script) {
        let params = vec![Param::String(script.to_electrum_scripthash().to_hex())];
        self.calls
            .push((String::from("blockchain.scripthash.get_balance"), params));
    }

    pub fn transaction_get(&mut self, tx_hash: &Txid) {
        let params = vec![Param::String(tx_hash.to_hex())];
        self.calls
            .push((String::from("blockchain.transaction.get"), params));
    }
}

impl std::iter::IntoIterator for Batch {
    type Item = (String, Vec<Param>);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.calls.into_iter()
    }
}

impl std::default::Default for Batch {
    fn default() -> Self {
        Batch { calls: Vec::new() }
    }
}
