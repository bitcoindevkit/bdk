use std::collections::HashMap;

use bdk_chain::{tx_graph::TxGraph, ConfirmationHeightAnchor, SpkTxOutIndex};
use bitcoin::{
    hashes::Hash, locktime::absolute::LockTime, secp256k1::Secp256k1, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use miniscript::Descriptor;

/// Transaction template.
pub struct TxTemplate<'a, A> {
    /// Uniquely identifies the transaction, before it can have a txid.
    pub tx_name: &'a str,
    pub inputs: &'a [TxInTemplate<'a>],
    pub outputs: &'a [TxOutTemplate],
    pub anchors: &'a [A],
    pub last_seen: Option<u64>,
}

impl<'a, A> Default for TxTemplate<'a, A> {
    fn default() -> Self {
        Self {
            tx_name: Default::default(),
            inputs: Default::default(),
            outputs: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
        }
    }
}

impl<'a, A> Copy for TxTemplate<'a, A> {}

impl<'a, A> Clone for TxTemplate<'a, A> {
    fn clone(&self) -> Self {
        *self
    }
}

#[allow(dead_code)]
pub enum TxInTemplate<'a> {
    /// This will give a random txid and vout.
    Bogus,

    /// This is used for coinbase transactions because they do not have previous outputs.
    Coinbase,

    /// Contains the `tx_name` and `vout` that we are spending. The rule is that we must only spend
    /// a previous transaction.
    PrevTx(&'a str, usize),
}

pub struct TxOutTemplate {
    pub value: u64,
    pub spk_index: Option<u32>, // some = get spk from SpkTxOutIndex, none = random spk
}

#[allow(dead_code)]
pub fn init_graph<'a>(
    tx_templates: impl IntoIterator<Item = &'a TxTemplate<'a, ConfirmationHeightAnchor>>,
) -> (
    TxGraph<ConfirmationHeightAnchor>,
    SpkTxOutIndex<u32>,
    HashMap<&'a str, Txid>,
) {
    let (descriptor, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)").unwrap();
    let mut graph = TxGraph::<ConfirmationHeightAnchor>::default();
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
            version: 0,
            lock_time: LockTime::ZERO,
            input: tx_tmp
                .inputs
                .iter()
                .map(|input| match input {
                    TxInTemplate::Bogus => TxIn {
                        // #TODO have actual random data
                        previous_output: OutPoint::new(Txid::all_zeros(), bogus_txin_vout as u32),
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
                        value: output.value,
                        script_pubkey: ScriptBuf::new(),
                    },
                    Some(index) => TxOut {
                        value: output.value,
                        script_pubkey: spk_index.spk_at_index(index).unwrap().to_owned(),
                    },
                })
                .collect(),
        };

        tx_ids.insert(tx_tmp.tx_name, tx.txid());
        spk_index.scan(&tx);
        let _ = graph.insert_tx(tx.clone());
        for anchor in tx_tmp.anchors.iter() {
            let _ = graph.insert_anchor(tx.txid(), *anchor);
        }
        if let Some(seen_at) = tx_tmp.last_seen {
            let _ = graph.insert_seen_at(tx.txid(), seen_at);
        }
    }
    (graph, spk_index, tx_ids)
}
