mod common;

use bdk_chain::{
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    tx_graph::Additions,
    BlockId, SpkTxOutIndex,
};
use bitcoin::{hashes::hex::FromHex, OutPoint, Script, Transaction, TxIn, TxOut};

/// Ensure [`IndexedTxGraph::insert_relevant_txs`] can successfully index transactions NOT presented
/// in topological order.
///
/// Given 3 transactions (A, B, C), where A has 2 owned outputs. B and C spends an output each of A.
/// Typically, we would only know whether B and C are relevant if we have indexed A (A's outpoints
/// are associated with owned spks in the index). Ensure insertion and indexing is topological-
/// agnostic.
#[test]
fn insert_relevant_txs() {
    let mut graph = IndexedTxGraph::<BlockId, SpkTxOutIndex<u32>>::default();

    // insert some spks
    let spk_0 = Script::from_hex("0014034f9515cace31713707dff8194b8f550eb6d336").unwrap();
    let spk_1 = Script::from_hex("0014beaa39ab2b4f47995c77107d8c3f481d3bd33941").unwrap();
    graph.index.insert_spk(0, spk_0.clone());
    graph.index.insert_spk(1, spk_1.clone());

    let tx_a = Transaction {
        output: vec![
            TxOut {
                value: 10_000,
                script_pubkey: spk_0,
            },
            TxOut {
                value: 20_000,
                script_pubkey: spk_1,
            },
        ],
        ..common::new_tx(0)
    };

    let tx_b = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 0),
            ..Default::default()
        }],
        ..common::new_tx(1)
    };

    let tx_c = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.txid(), 1),
            ..Default::default()
        }],
        ..common::new_tx(2)
    };

    let txs = [tx_c, tx_b, tx_a];

    assert_eq!(
        graph.insert_relevant_txs(&txs, None, None),
        IndexedAdditions {
            graph_additions: Additions {
                tx: txs.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    )
}
