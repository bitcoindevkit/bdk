#![cfg(feature = "miniscript")]

use bdk_testenv::utils::DESCRIPTORS;
use rand::distributions::{Alphanumeric, DistString};
use std::collections::HashMap;

use bdk_chain::{spk_txout::SpkTxOutIndex, tx_graph::TxGraph, Anchor};
use bitcoin::{
    locktime::absolute::LockTime, secp256k1::Secp256k1, transaction, Amount, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use miniscript::Descriptor;

/// Template for creating a transaction in `TxGraph`.
///
/// The incentive for transaction templates is to create a transaction history in a simple manner to
/// avoid having to explicitly hash previous transactions to form previous outpoints of later
/// transactions.
#[derive(Clone, Copy, Default)]
pub struct TxTemplate<'a, A> {
    /// Uniquely identifies the transaction, before it can have a txid.
    pub tx_name: &'a str,
    pub inputs: &'a [TxInTemplate<'a>],
    pub outputs: &'a [TxOutTemplate],
    pub anchors: &'a [A],
    pub last_seen: Option<u64>,
}

#[allow(dead_code)]
pub enum TxInTemplate<'a> {
    /// This will give a random txid and vout.
    Bogus,

    /// This is used for coinbase transactions because they do not have previous outputs.
    Coinbase,

    /// Contains the `tx_name` and `vout` that we are spending. The rule is that we must only spend
    /// from tx of a previous `TxTemplate`.
    PrevTx(&'a str, usize),
}

pub struct TxOutTemplate {
    pub value: u64,
    pub spk_index: Option<u32>, // some = get spk from SpkTxOutIndex, none = random spk
}

#[allow(unused)]
impl TxOutTemplate {
    pub fn new(value: u64, spk_index: Option<u32>) -> Self {
        TxOutTemplate { value, spk_index }
    }
}

#[allow(dead_code)]
pub fn init_graph<'a, A: Anchor + Clone + 'a>(
    tx_templates: impl IntoIterator<Item = &'a TxTemplate<'a, A>>,
) -> (TxGraph<A>, SpkTxOutIndex<u32>, HashMap<&'a str, Txid>) {
    let (descriptor, _) =
        Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[2]).unwrap();
    let mut graph = TxGraph::<A>::default();
    let mut spk_index = SpkTxOutIndex::default();
    (0..10).for_each(|index| {
        spk_index.insert_spk(
            index,
            descriptor
                .at_derivation_index(index)
                .unwrap()
                .script_pubkey(),
        );
    });
    let mut tx_ids = HashMap::<&'a str, Txid>::new();

    for (bogus_txin_vout, tx_tmp) in tx_templates.into_iter().enumerate() {
        let tx = Transaction {
            version: transaction::Version::non_standard(0),
            lock_time: LockTime::ZERO,
            input: tx_tmp
                .inputs
                .iter()
                .map(|input| match input {
                    TxInTemplate::Bogus => TxIn {
                        previous_output: OutPoint::new(
                            bitcoin::hashes::Hash::hash(
                                Alphanumeric
                                    .sample_string(&mut rand::thread_rng(), 20)
                                    .as_bytes(),
                            ),
                            bogus_txin_vout as u32,
                        ),
                        script_sig: ScriptBuf::new(),
                        sequence: Sequence::default(),
                        witness: Witness::new(),
                    },
                    TxInTemplate::Coinbase => TxIn {
                        previous_output: OutPoint::null(),
                        script_sig: ScriptBuf::new(),
                        sequence: Sequence::MAX,
                        witness: Witness::new(),
                    },
                    TxInTemplate::PrevTx(prev_name, prev_vout) => {
                        let prev_txid = tx_ids.get(prev_name).expect(
                            "txin template must spend from tx of template that comes before",
                        );
                        TxIn {
                            previous_output: OutPoint::new(*prev_txid, *prev_vout as _),
                            script_sig: ScriptBuf::new(),
                            sequence: Sequence::default(),
                            witness: Witness::new(),
                        }
                    }
                })
                .collect(),
            output: tx_tmp
                .outputs
                .iter()
                .map(|output| match &output.spk_index {
                    None => TxOut {
                        value: Amount::from_sat(output.value),
                        script_pubkey: ScriptBuf::new(),
                    },
                    Some(index) => TxOut {
                        value: Amount::from_sat(output.value),
                        script_pubkey: spk_index.spk_at_index(index).unwrap(),
                    },
                })
                .collect(),
        };

        tx_ids.insert(tx_tmp.tx_name, tx.compute_txid());
        spk_index.scan(&tx);
        let _ = graph.insert_tx(tx.clone());
        for anchor in tx_tmp.anchors.iter() {
            let _ = graph.insert_anchor(tx.compute_txid(), anchor.clone());
        }
        if let Some(last_seen) = tx_tmp.last_seen {
            let _ = graph.insert_seen_at(tx.compute_txid(), last_seen);
        }
    }
    (graph, spk_index, tx_ids)
}
