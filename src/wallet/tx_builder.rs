use std::collections::BTreeMap;

use bitcoin::{Address, OutPoint};

#[derive(Debug, Default)]
pub struct TxBuilder {
    pub(crate) addressees: Vec<(Address, u64)>,
    pub(crate) send_all: bool,
    pub(crate) fee_perkb: Option<f32>,
    pub(crate) policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) utxos: Option<Vec<OutPoint>>,
    pub(crate) unspendable: Option<Vec<OutPoint>>,
}

impl TxBuilder {
    pub fn new() -> TxBuilder {
        TxBuilder::default()
    }

    pub fn from_addressees(addressees: Vec<(Address, u64)>) -> TxBuilder {
        let mut tx_builder = TxBuilder::default();
        tx_builder.addressees = addressees;

        tx_builder
    }

    pub fn add_addressee(&mut self, address: Address, amount: u64) -> &mut TxBuilder {
        self.addressees.push((address, amount));
        self
    }

    pub fn send_all(&mut self) -> &mut TxBuilder {
        self.send_all = true;
        self
    }

    pub fn fee_rate(&mut self, satoshi_per_vbyte: f32) -> &mut TxBuilder {
        self.fee_perkb = Some(satoshi_per_vbyte * 1e3);
        self
    }

    pub fn fee_rate_perkb(&mut self, satoshi_per_kb: f32) -> &mut TxBuilder {
        self.fee_perkb = Some(satoshi_per_kb);
        self
    }

    pub fn policy_path(&mut self, policy_path: BTreeMap<String, Vec<usize>>) -> &mut TxBuilder {
        self.policy_path = Some(policy_path);
        self
    }

    pub fn utxos(&mut self, utxos: Vec<OutPoint>) -> &mut TxBuilder {
        self.utxos = Some(utxos);
        self
    }

    pub fn add_utxo(&mut self, utxo: OutPoint) -> &mut TxBuilder {
        self.utxos.get_or_insert(vec![]).push(utxo);
        self
    }

    pub fn unspendable(&mut self, unspendable: Vec<OutPoint>) -> &mut TxBuilder {
        self.unspendable = Some(unspendable);
        self
    }

    pub fn add_unspendable(&mut self, unspendable: OutPoint) -> &mut TxBuilder {
        self.unspendable.get_or_insert(vec![]).push(unspendable);
        self
    }
}
