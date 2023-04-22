mod common;

use bdk_chain::{
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{DerivationAdditions, KeychainTxOutIndex},
    tx_graph::Additions,
    BlockId,
};
use bitcoin::{secp256k1::Secp256k1, OutPoint, Transaction, TxIn, TxOut};
use miniscript::Descriptor;

/// Ensure [`IndexedTxGraph::insert_relevant_txs`] can successfully index transactions NOT presented
/// in topological order.
///
/// Given 3 transactions (A, B, C), where A has 2 owned outputs. B and C spends an output each of A.
/// Typically, we would only know whether B and C are relevant if we have indexed A (A's outpoints
/// are associated with owned spks in the index). Ensure insertion and indexing is topological-
/// agnostic.
#[test]
fn insert_relevant_txs() {
    const DESCRIPTOR: &str = "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)";
    let (descriptor, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTOR)
        .expect("must be valid");
    let spk_0 = descriptor.at_derivation_index(0).script_pubkey();
    let spk_1 = descriptor.at_derivation_index(9).script_pubkey();

    let mut graph = IndexedTxGraph::<BlockId, KeychainTxOutIndex<()>>::default();
    graph.index.add_keychain((), descriptor);
    graph.index.set_lookahead(&(), 10);

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
        graph.insert_relevant_txs(txs.iter().map(|tx| (tx, None)), None),
        IndexedAdditions {
            graph_additions: Additions {
                tx: txs.into(),
                ..Default::default()
            },
            index_additions: DerivationAdditions([((), 9_u32)].into()),
        }
    )
}
