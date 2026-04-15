//! Transaction templates for constructing complex transaction histories for testing purposes.

use crate::utils::DESCRIPTORS;
use bdk_chain::{
    miniscript::Descriptor, spk_txout::SpkTxOutIndex, tx_graph::TxGraph, Anchor,
    CanonicalizationParams,
};
use bitcoin::{
    locktime::absolute::LockTime, secp256k1::Secp256k1, transaction, Amount, OutPoint, ScriptBuf,
    Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use rand::distributions::{Alphanumeric, DistString};
use std::{borrow::Cow, collections::HashMap};

/// Template for creating a transaction in a [`TxGraph`].
///
/// This is the main building block for constructing complex transaction histories
/// for tests. It allows you to refer to previous transactions by name instead of
/// manually managing txids and outpoints.
#[derive(Clone)]
pub struct TxTemplate<A>
where
    A: Clone + 'static,
{
    /// A unique name used to refer to this transaction in other templates.
    pub tx_name: Cow<'static, str>,

    /// The inputs of this transaction.
    pub inputs: Cow<'static, [TxInTemplate]>,

    /// The outputs of this transaction.
    pub outputs: Cow<'static, [TxOutTemplate]>,

    /// Anchors (confirmations) for this transaction.
    pub anchors: Cow<'static, [A]>,

    /// Unix timestamp when this transaction was last seen in the mempool.
    pub last_seen: Option<u64>,

    /// If `true`, this transaction will be treated as canonical regardless of
    /// conflict resolution rules (used for testing forced canonicalization).
    pub assume_canonical: bool,
}

impl<A> Default for TxTemplate<A>
where
    A: Clone + 'static,
{
    fn default() -> Self {
        Self {
            tx_name: Cow::Borrowed(""),
            inputs: Cow::Borrowed(&[]),
            outputs: Cow::Borrowed(&[]),
            anchors: Cow::Borrowed(&[]),
            last_seen: None,
            assume_canonical: false,
        }
    }
}

impl<A> TxTemplate<A>
where
    A: Clone + 'static,
{
    /// Create a new template with a name.
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            tx_name: name.into(),
            ..Default::default()
        }
    }

    //// Set inputs. Accepts `&[TxInTemplate]`, `Vec<TxInTemplate>`, or a static slice.
    pub fn with_inputs(mut self, inputs: impl Into<Cow<'static, [TxInTemplate]>>) -> Self {
        self.inputs = inputs.into();
        self
    }

    /// Set outputs with `Vec<TxOutTemplate>` or `&'static [TxOutTemplate]`.
    pub fn with_outputs(mut self, outputs: impl Into<Cow<'static, [TxOutTemplate]>>) -> Self {
        self.outputs = outputs.into();
        self
    }

    /// Set anchors. Supports `Vec<A>` or `&'static [A]`.
    pub fn with_anchors(mut self, anchors: impl Into<Cow<'static, [A]>>) -> Self {
        self.anchors = anchors.into();
        self
    }

    /// Mark this transaction as canonical.
    pub fn with_assume_canonical(mut self, assume: bool) -> Self {
        self.assume_canonical = assume;
        self
    }

    /// Set the last-seen mempool timestamp.
    pub fn with_last_seen(mut self, last_seen: Option<u64>) -> Self {
        self.last_seen = last_seen;
        self
    }
}

/// Describes how an input is created in a [`TxTemplate`].
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum TxInTemplate {
    /// A random (bogus) previous output. Useful when the actual prevout doesn't matter.
    Bogus,

    /// A coinbase input (no previous output).
    Coinbase,

    /// Spends from a previous transaction defined in the template list.
    ///
    /// The rule is that the referenced transaction (`prev_name`) must appear
    /// earlier in the list passed to [`init_graph`].
    PrevTx(Cow<'static, str>, usize),
}

/// Describes an output in a [`TxTemplate`].
#[derive(Clone, Debug)]
pub struct TxOutTemplate {
    /// Value in satoshis.
    pub value: u64,
    /// If `Some(index)`, the output will use the script pubkey at that index
    /// from the test descriptor set. If `None`, a random (empty) script is used.
    pub spk_index: Option<u32>,
}

impl TxOutTemplate {
    pub fn new(value: u64, spk_index: Option<u32>) -> Self {
        TxOutTemplate { value, spk_index }
    }
}

/// The result of calling [`init_graph`].
///
/// Contains the built [`TxGraph`], the associated indexer, and a mapping from
/// template names to their final txids.
#[allow(dead_code)]
pub struct TxTemplateEnv<A> {
    pub tx_graph: TxGraph<A>,
    pub indexer: SpkTxOutIndex<u32>,
    pub txid_to_name: HashMap<Cow<'static, str>, Txid>,
    pub canonicalization_params: CanonicalizationParams,
}

/// Builds a [`TxGraph`] (and associated indexer) from a list of [`TxTemplate`]s.
///
/// This is the main entry point for using transaction templates in tests.
/// It handles txid generation, outpoint wiring, anchor insertion, and last-seen
/// timestamps automatically.
pub fn init_graph<A: Anchor + Clone + 'static>(
    tx_templates: impl IntoIterator<Item = TxTemplate<A>>,
) -> TxTemplateEnv<A> {
    let (descriptor, _) =
        Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[2]).unwrap();

    let mut tx_graph = TxGraph::<A>::default();
    let mut indexer = SpkTxOutIndex::default();

    // Pre-populate the indexer with 10 script pubkeys from the test descriptor
    (0..10).for_each(|index| {
        indexer.insert_spk(
            index,
            descriptor
                .at_derivation_index(index)
                .unwrap()
                .script_pubkey(),
        );
    });

    let mut txid_to_name = HashMap::<Cow<'static, str>, Txid>::new();
    let mut canonicalization_params = CanonicalizationParams::default();

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
        let _ = tx_graph.insert_tx(tx.clone());

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
