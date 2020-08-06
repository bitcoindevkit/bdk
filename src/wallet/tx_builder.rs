use std::collections::BTreeMap;

use bitcoin::{Address, OutPoint, SigHashType};

use super::coin_selection::{CoinSelectionAlgorithm, DefaultCoinSelectionAlgorithm};

// TODO: add a flag to ignore change outputs (make them unspendable)
#[derive(Debug, Default)]
pub struct TxBuilder<Cs: CoinSelectionAlgorithm> {
    pub(crate) addressees: Vec<(Address, u64)>,
    pub(crate) send_all: bool,
    pub(crate) fee_perkb: Option<f32>,
    pub(crate) policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) utxos: Option<Vec<OutPoint>>,
    pub(crate) unspendable: Option<Vec<OutPoint>>,
    pub(crate) sighash: Option<SigHashType>,
    pub(crate) shuffle_outputs: Option<bool>,
    pub(crate) coin_selection: Cs,
}

impl TxBuilder<DefaultCoinSelectionAlgorithm> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_addressees(addressees: Vec<(Address, u64)>) -> Self {
        Self::default().set_addressees(addressees)
    }
}

impl<Cs: CoinSelectionAlgorithm> TxBuilder<Cs> {
    pub fn set_addressees(mut self, addressees: Vec<(Address, u64)>) -> Self {
        self.addressees = addressees;
        self
    }

    pub fn add_addressee(mut self, address: Address, amount: u64) -> Self {
        self.addressees.push((address, amount));
        self
    }

    pub fn send_all(mut self, send_all: bool) -> Self {
        self.send_all = send_all;
        self
    }

    pub fn fee_rate(mut self, satoshi_per_vbyte: f32) -> Self {
        self.fee_perkb = Some(satoshi_per_vbyte * 1e3);
        self
    }

    pub fn fee_rate_perkb(mut self, satoshi_per_kb: f32) -> Self {
        self.fee_perkb = Some(satoshi_per_kb);
        self
    }

    pub fn policy_path(mut self, policy_path: BTreeMap<String, Vec<usize>>) -> Self {
        self.policy_path = Some(policy_path);
        self
    }

    pub fn utxos(mut self, utxos: Vec<OutPoint>) -> Self {
        self.utxos = Some(utxos);
        self
    }

    pub fn add_utxo(mut self, utxo: OutPoint) -> Self {
        self.utxos.get_or_insert(vec![]).push(utxo);
        self
    }

    pub fn unspendable(mut self, unspendable: Vec<OutPoint>) -> Self {
        self.unspendable = Some(unspendable);
        self
    }

    pub fn add_unspendable(mut self, unspendable: OutPoint) -> Self {
        self.unspendable.get_or_insert(vec![]).push(unspendable);
        self
    }

    pub fn sighash(mut self, sighash: SigHashType) -> Self {
        self.sighash = Some(sighash);
        self
    }

    pub fn do_not_shuffle_outputs(mut self) -> Self {
        self.shuffle_outputs = Some(false);
        self
    }

    pub fn coin_selection<P: CoinSelectionAlgorithm>(self, coin_selection: P) -> TxBuilder<P> {
        TxBuilder {
            addressees: self.addressees,
            send_all: self.send_all,
            fee_perkb: self.fee_perkb,
            policy_path: self.policy_path,
            utxos: self.utxos,
            unspendable: self.unspendable,
            sighash: self.sighash,
            shuffle_outputs: self.shuffle_outputs,
            coin_selection,
        }
    }
}
