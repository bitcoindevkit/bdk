//! Transaction templates for constructing complex transaction histories for testing purposes.

use crate::utils::DESCRIPTORS;
use bdk_chain::{
    miniscript::Descriptor, spk_txout::SpkTxOutIndex, tx_graph::TxGraph, Anchor, CanonicalParams,
};
use bitcoin::{
    locktime::absolute::LockTime, secp256k1::Secp256k1, transaction, Amount, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use std::collections::HashMap;

/// Template for creating a transaction in a [`TxGraph`].
///
/// This is the main building block for constructing complex transaction histories
/// for tests. It allows you to refer to previous transactions by name instead of
/// manually managing txids and outpoints.
#[derive(Clone)]
pub struct TxTemplate<A> {
    /// A unique name used to refer to this transaction in other templates.
    pub tx_name: &'static str,

    /// The inputs of this transaction.
    pub inputs: Vec<TxInTemplate>,

    /// The outputs of this transaction.
    pub outputs: Vec<TxOutTemplate>,

    /// Anchors (confirmations) for this transaction.
    pub anchors: Vec<A>,

    /// Unix timestamp when this transaction was last seen in the mempool.
    pub last_seen: Option<u64>,

    /// If `true`, this transaction will be treated as canonical regardless of
    /// conflict resolution rules (used for testing forced canonicalization).
    pub assume_canonical: bool,
}

/// Describes how an input is created in a [`TxTemplate`].
#[derive(Clone, Debug)]
pub enum TxInTemplate {
    /// A random (bogus) previous output. Useful when the actual prevout doesn't matter.
    Bogus,

    /// A coinbase input (no previous output).
    Coinbase,

    /// Spends from a previous transaction defined in the template list.
    ///
    /// - `0` (`&'static str`): the `tx_name` of the transaction to spend from. It must appear
    ///   earlier in the list passed to [`init_graph`] (otherwise `init_graph` panics).
    /// - `1` (`usize`): the output index (vout) of that transaction to spend.
    PrevTx(&'static str, usize),
}

/// Describes an output in a [`TxTemplate`].
#[derive(Clone, Copy, Debug)]
pub struct TxOutTemplate {
    /// Value in satoshis.
    pub value: u64,
    /// If `Some(index)`, the output will use the script pubkey at that index
    /// from the test descriptor set. If `None`, a random (empty) script is used.
    pub spk_index: Option<u32>,
}

impl<A> TxTemplate<A> {
    /// Create a new template with a name.
    pub fn new(tx_name: &'static str) -> Self {
        Self {
            tx_name,
            inputs: Vec::new(),
            outputs: Vec::new(),
            anchors: Vec::new(),
            last_seen: None,
            assume_canonical: false,
        }
    }

    /// Set the inputs of this transaction.
    pub fn with_inputs(mut self, inputs: impl IntoIterator<Item = TxInTemplate>) -> Self {
        self.inputs = inputs.into_iter().collect();
        self
    }

    /// Set the outputs of this transaction.
    pub fn with_outputs(mut self, outputs: impl IntoIterator<Item = TxOutTemplate>) -> Self {
        self.outputs = outputs.into_iter().collect();
        self
    }

    /// Set the anchors (confirmations) of this transaction.
    pub fn with_anchors(mut self, anchors: impl IntoIterator<Item = A>) -> Self {
        self.anchors = anchors.into_iter().collect();
        self
    }

    /// Mark this transaction as canonical regardless of conflict resolution rules.
    pub fn assume_canonical(mut self) -> Self {
        self.assume_canonical = true;
        self
    }

    /// Set the last-seen mempool timestamp.
    pub fn with_last_seen(mut self, last_seen: u64) -> Self {
        self.last_seen = Some(last_seen);
        self
    }
}

impl TxOutTemplate {
    /// Create an output of `value` sats. `spk_index` selects a script pubkey
    /// from the test descriptor set, or `None` for an empty script.
    pub fn new(value: u64, spk_index: Option<u32>) -> Self {
        TxOutTemplate { value, spk_index }
    }
}

/// The result of calling [`init_graph`].
///
/// Contains the built [`TxGraph`], the associated indexer, and a mapping from
/// template names to their final txids.
pub struct TxTemplateEnv<A> {
    /// The graph built from the templates.
    pub tx_graph: TxGraph<A>,
    /// Indexer holding the test descriptor's script pubkeys, scanned
    /// against every built transaction.
    pub indexer: SpkTxOutIndex<u32>,
    /// Maps each template's `tx_name` to the txid it was assigned.
    pub txid_to_name: HashMap<&'static str, Txid>,
    /// Canonicalization params, pre-populated with the txids of every template
    /// marked [`assume_canonical`](TxTemplate::assume_canonical).
    pub canonicalization_params: CanonicalParams,
}

/// Builds a [`TxGraph`] (and associated indexer) from a list of [`TxTemplate`]s.
///
/// This is the main entry point for using transaction templates in tests.
/// It handles txid generation, outpoint wiring, anchor insertion, and last-seen
/// timestamps automatically.
pub fn init_graph<A: Anchor>(
    tx_templates: impl IntoIterator<Item = TxTemplate<A>>,
) -> TxTemplateEnv<A> {
    let (descriptor, _) =
        Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[2]).unwrap();
    let mut tx_graph = TxGraph::<A>::default();
    let mut indexer = SpkTxOutIndex::default();
    let mut txid_to_name = HashMap::<&'static str, Txid>::new();
    let mut canonicalization_params = CanonicalParams::default();

    for (bogus_txin_vout, tx_tmp) in tx_templates.into_iter().enumerate() {
        for output in &tx_tmp.outputs {
            if let Some(index) = output.spk_index {
                if indexer.spk_at_index(&index).is_none() {
                    indexer.insert_spk(
                        index,
                        descriptor
                            .at_derivation_index(index)
                            .unwrap()
                            .script_pubkey(),
                    );
                }
            }
        }
        let tx = Transaction {
            version: transaction::Version::non_standard(0),
            lock_time: LockTime::ZERO,
            input: tx_tmp
                .inputs
                .iter()
                .enumerate()
                .map(|(input_i, input)| match input {
                    TxInTemplate::Bogus => TxIn {
                        previous_output: OutPoint::new(
                            bitcoin::hashes::Hash::hash(
                                format!("bogus-{bogus_txin_vout}-{input_i}").as_bytes(),
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
                        let prev_txid = txid_to_name.get(prev_name).expect(
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
                        script_pubkey: indexer.spk_at_index(index).unwrap(),
                    },
                })
                .collect(),
        };

        let txid = tx.compute_txid();
        if tx_tmp.assume_canonical {
            canonicalization_params.assume_canonical.push(txid);
        }
        txid_to_name.insert(tx_tmp.tx_name, txid);
        indexer.scan(&tx);
        let _ = tx_graph.insert_tx(tx);
        for anchor in tx_tmp.anchors.iter() {
            let _ = tx_graph.insert_anchor(txid, anchor.clone());
        }
        if let Some(last_seen) = tx_tmp.last_seen {
            let _ = tx_graph.insert_seen_at(txid, last_seen);
        }
    }
    TxTemplateEnv {
        tx_graph,
        indexer,
        txid_to_name,
        canonicalization_params,
    }
}
