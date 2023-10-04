#[macro_use]
mod common;

use std::collections::{BTreeMap, BTreeSet};

use bdk_chain::{
    indexed_tx_graph::{self, IndexedTxGraph},
    keychain::{self, Balance, KeychainTxOutIndex},
    local_chain::LocalChain,
    tx_graph, BlockId, ChainPosition, ConfirmationHeightAnchor,
};
use bitcoin::{
    secp256k1::Secp256k1, BlockHash, OutPoint, Script, ScriptBuf, Transaction, TxIn, TxOut,
};
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
    let spk_0 = descriptor.at_derivation_index(0).unwrap().script_pubkey();
    let spk_1 = descriptor.at_derivation_index(9).unwrap().script_pubkey();

    let mut graph = IndexedTxGraph::<ConfirmationHeightAnchor, KeychainTxOutIndex<()>>::default();
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

    let changeset = indexed_tx_graph::ChangeSet {
        graph: tx_graph::ChangeSet {
            txs: txs.clone().into(),
            ..Default::default()
        },
        indexer: keychain::ChangeSet([((), 9_u32)].into()),
    };

    assert_eq!(
        graph.batch_insert_relevant(txs.iter().map(|tx| (tx, None, None))),
        changeset,
    );

    assert_eq!(graph.initial_changeset(), changeset,);
}

#[test]
/// Ensure consistency IndexedTxGraph list_* and balance methods. These methods lists
/// relevant txouts and utxos from the information fetched from a ChainOracle (here a LocalChain).
///
/// Test Setup:
///
/// Local Chain =>  <0> ----- <1> ----- <2> ----- <3> ---- ... ---- <150>
///
/// Keychains:
///
/// keychain_1: Trusted
/// keychain_2: Untrusted
///
/// Transactions:
///
/// tx1: A Coinbase, sending 70000 sats to "trusted" address. [Block 0]
/// tx2: A external Receive, sending 30000 sats to "untrusted" address. [Block 1]
/// tx3: Internal Spend. Spends tx2 and returns change of 10000 to "trusted" address. [Block 2]
/// tx4: Mempool tx, sending 20000 sats to "trusted" address.
/// tx5: Mempool tx, sending 15000 sats to "untested" address.
/// tx6: Complete unrelated tx. [Block 3]
///
/// Different transactions are added via `insert_relevant_txs`.
/// `list_owned_txout`, `list_owned_utxos` and `balance` method is asserted
/// with expected values at Block height 0, 1, and 2.
///
/// Finally Add more blocks to local chain until tx1 coinbase maturity hits.
/// Assert maturity at coinbase maturity inflection height. Block height 98 and 99.

fn test_list_owned_txouts() {
    // Create Local chains
    let local_chain = LocalChain::from(
        (0..150)
            .map(|i| (i as u32, h!("random")))
            .collect::<BTreeMap<u32, BlockHash>>(),
    );

    // Initiate IndexedTxGraph

    let (desc_1, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)").unwrap();
    let (desc_2, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/1/*)").unwrap();

    let mut graph =
        IndexedTxGraph::<ConfirmationHeightAnchor, KeychainTxOutIndex<String>>::default();

    graph.index.add_keychain("keychain_1".into(), desc_1);
    graph.index.add_keychain("keychain_2".into(), desc_2);
    graph.index.set_lookahead_for_all(10);

    // Get trusted and untrusted addresses

    let mut trusted_spks: Vec<ScriptBuf> = Vec::new();
    let mut untrusted_spks: Vec<ScriptBuf> = Vec::new();

    {
        // we need to scope here to take immutanble reference of the graph
        for _ in 0..10 {
            let ((_, script), _) = graph.index.reveal_next_spk(&"keychain_1".to_string());
            // TODO Assert indexes
            trusted_spks.push(script.to_owned());
        }
    }
    {
        for _ in 0..10 {
            let ((_, script), _) = graph.index.reveal_next_spk(&"keychain_2".to_string());
            untrusted_spks.push(script.to_owned());
        }
    }

    // Create test transactions

    // tx1 is the genesis coinbase
    let tx1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 70000,
            script_pubkey: trusted_spks[0].to_owned(),
        }],
        ..common::new_tx(0)
    };

    // tx2 is an incoming transaction received at untrusted keychain at block 1.
    let tx2 = Transaction {
        output: vec![TxOut {
            value: 30000,
            script_pubkey: untrusted_spks[0].to_owned(),
        }],
        ..common::new_tx(0)
    };

    // tx3 spends tx2 and gives a change back in trusted keychain. Confirmed at Block 2.
    let tx3 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx2.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 10000,
            script_pubkey: trusted_spks[1].to_owned(),
        }],
        ..common::new_tx(0)
    };

    // tx4 is an external transaction receiving at untrusted keychain, unconfirmed.
    let tx4 = Transaction {
        output: vec![TxOut {
            value: 20000,
            script_pubkey: untrusted_spks[1].to_owned(),
        }],
        ..common::new_tx(0)
    };

    // tx5 is spending tx3 and receiving change at trusted keychain, unconfirmed.
    let tx5 = Transaction {
        output: vec![TxOut {
            value: 15000,
            script_pubkey: trusted_spks[2].to_owned(),
        }],
        ..common::new_tx(0)
    };

    // tx6 is an unrelated transaction confirmed at 3.
    let tx6 = common::new_tx(0);

    // Insert transactions into graph with respective anchors
    // For unconfirmed txs we pass in `None`.

    let _ =
        graph.batch_insert_relevant([&tx1, &tx2, &tx3, &tx6].iter().enumerate().map(|(i, tx)| {
            let height = i as u32;
            (
                *tx,
                local_chain
                    .blocks()
                    .get(&height)
                    .cloned()
                    .map(|hash| BlockId { height, hash })
                    .map(|anchor_block| ConfirmationHeightAnchor {
                        anchor_block,
                        confirmation_height: anchor_block.height,
                    }),
                None,
            )
        }));

    let _ = graph.batch_insert_relevant([&tx4, &tx5].iter().map(|tx| (*tx, None, Some(100))));

    // A helper lambda to extract and filter data from the graph.
    let fetch =
        |height: u32,
         graph: &IndexedTxGraph<ConfirmationHeightAnchor, KeychainTxOutIndex<String>>| {
            let chain_tip = local_chain
                .blocks()
                .get(&height)
                .map(|&hash| BlockId { height, hash })
                .unwrap_or_else(|| panic!("block must exist at {}", height));
            let txouts = graph
                .graph()
                .filter_chain_txouts(
                    &local_chain,
                    chain_tip,
                    graph.index.outpoints().iter().cloned(),
                )
                .collect::<Vec<_>>();

            let utxos = graph
                .graph()
                .filter_chain_unspents(
                    &local_chain,
                    chain_tip,
                    graph.index.outpoints().iter().cloned(),
                )
                .collect::<Vec<_>>();

            let balance = graph.graph().balance(
                &local_chain,
                chain_tip,
                graph.index.outpoints().iter().cloned(),
                |_, spk: &Script| trusted_spks.contains(&spk.to_owned()),
            );

            assert_eq!(txouts.len(), 5);
            assert_eq!(utxos.len(), 4);

            let confirmed_txouts_txid = txouts
                .iter()
                .filter_map(|(_, full_txout)| {
                    if matches!(full_txout.chain_position, ChainPosition::Confirmed(_)) {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            let unconfirmed_txouts_txid = txouts
                .iter()
                .filter_map(|(_, full_txout)| {
                    if matches!(full_txout.chain_position, ChainPosition::Unconfirmed(_)) {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            let confirmed_utxos_txid = utxos
                .iter()
                .filter_map(|(_, full_txout)| {
                    if matches!(full_txout.chain_position, ChainPosition::Confirmed(_)) {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            let unconfirmed_utxos_txid = utxos
                .iter()
                .filter_map(|(_, full_txout)| {
                    if matches!(full_txout.chain_position, ChainPosition::Unconfirmed(_)) {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            (
                confirmed_txouts_txid,
                unconfirmed_txouts_txid,
                confirmed_utxos_txid,
                unconfirmed_utxos_txid,
                balance,
            )
        };

    // ----- TEST BLOCK -----

    // AT Block 0
    {
        let (
            confirmed_txouts_txid,
            unconfirmed_txouts_txid,
            confirmed_utxos_txid,
            unconfirmed_utxos_txid,
            balance,
        ) = fetch(0, &graph);

        assert_eq!(confirmed_txouts_txid, [tx1.txid()].into());
        assert_eq!(
            unconfirmed_txouts_txid,
            [tx2.txid(), tx3.txid(), tx4.txid(), tx5.txid()].into()
        );

        assert_eq!(confirmed_utxos_txid, [tx1.txid()].into());
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx3.txid(), tx4.txid(), tx5.txid()].into()
        );

        assert_eq!(
            balance,
            Balance {
                immature: 70000,          // immature coinbase
                trusted_pending: 25000,   // tx3 + tx5
                untrusted_pending: 20000, // tx4
                confirmed: 0              // Nothing is confirmed yet
            }
        );
    }

    // AT Block 1
    {
        let (
            confirmed_txouts_txid,
            unconfirmed_txouts_txid,
            confirmed_utxos_txid,
            unconfirmed_utxos_txid,
            balance,
        ) = fetch(1, &graph);

        // tx2 gets into confirmed txout set
        assert_eq!(confirmed_txouts_txid, [tx1.txid(), tx2.txid()].into());
        assert_eq!(
            unconfirmed_txouts_txid,
            [tx3.txid(), tx4.txid(), tx5.txid()].into()
        );

        // tx2 doesn't get into confirmed utxos set
        assert_eq!(confirmed_utxos_txid, [tx1.txid()].into());
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx3.txid(), tx4.txid(), tx5.txid()].into()
        );

        assert_eq!(
            balance,
            Balance {
                immature: 70000,          // immature coinbase
                trusted_pending: 25000,   // tx3 + tx5
                untrusted_pending: 20000, // tx4
                confirmed: 0              // Nothing is confirmed yet
            }
        );
    }

    // AT Block 2
    {
        let (
            confirmed_txouts_txid,
            unconfirmed_txouts_txid,
            confirmed_utxos_txid,
            unconfirmed_utxos_txid,
            balance,
        ) = fetch(2, &graph);

        // tx3 now gets into the confirmed txout set
        assert_eq!(
            confirmed_txouts_txid,
            [tx1.txid(), tx2.txid(), tx3.txid()].into()
        );
        assert_eq!(unconfirmed_txouts_txid, [tx4.txid(), tx5.txid()].into());

        // tx3 also gets into confirmed utxo set
        assert_eq!(confirmed_utxos_txid, [tx1.txid(), tx3.txid()].into());
        assert_eq!(unconfirmed_utxos_txid, [tx4.txid(), tx5.txid()].into());

        assert_eq!(
            balance,
            Balance {
                immature: 70000,          // immature coinbase
                trusted_pending: 15000,   // tx5
                untrusted_pending: 20000, // tx4
                confirmed: 10000          // tx3 got confirmed
            }
        );
    }

    // AT Block 98
    {
        let (
            confirmed_txouts_txid,
            unconfirmed_txouts_txid,
            confirmed_utxos_txid,
            unconfirmed_utxos_txid,
            balance,
        ) = fetch(98, &graph);

        assert_eq!(
            confirmed_txouts_txid,
            [tx1.txid(), tx2.txid(), tx3.txid()].into()
        );
        assert_eq!(unconfirmed_txouts_txid, [tx4.txid(), tx5.txid()].into());

        assert_eq!(confirmed_utxos_txid, [tx1.txid(), tx3.txid()].into());
        assert_eq!(unconfirmed_utxos_txid, [tx4.txid(), tx5.txid()].into());

        // Coinbase is still immature
        assert_eq!(
            balance,
            Balance {
                immature: 70000,          // immature coinbase
                trusted_pending: 15000,   // tx5
                untrusted_pending: 20000, // tx4
                confirmed: 10000          // tx1 got matured
            }
        );
    }

    // AT Block 99
    {
        let (_, _, _, _, balance) = fetch(100, &graph);

        // Coinbase maturity hits
        assert_eq!(
            balance,
            Balance {
                immature: 0,              // coinbase matured
                trusted_pending: 15000,   // tx5
                untrusted_pending: 20000, // tx4
                confirmed: 80000          // tx1 + tx3
            }
        );
    }
}
