#[macro_use]
mod common;

use std::collections::{BTreeMap, BTreeSet, HashSet};

use bdk_chain::{
    indexed_tx_graph::{self, IndexedTxGraph},
    keychain::{self, Balance, KeychainTxOutIndex},
    local_chain::LocalChain,
    tx_graph, BlockId, ChainPosition, ConfirmationHeightAnchor, SpkIterator,
};
use bitcoin::{
    hashes::Hash, secp256k1::Secp256k1, BlockHash, OutPoint, Script, ScriptBuf, Transaction, TxIn,
    TxOut,
};
use common::*;
use miniscript::{Descriptor, DescriptorPublicKey};

#[allow(dead_code)]
struct Scenario<'a> {
    /// Name of the test scenario
    name: &'a str,
    /// Transaction templates
    tx_templates: &'a [TxTemplate<'a, ConfirmationHeightAnchor>],
    /// Names of txs that must exist in the output of `list_chain_txs`
    exp_chain_txs: HashSet<(&'a str, u32)>,
    /// Outpoints of UTXOs that must exist in the output of `filter_chain_unspents`
    exp_unspents: HashSet<(&'a str, u32)>,
    /// Expected balances
    exp_balance: Balance,
}

/// This test ensures that [`TxGraph`] will reliably filter out irrelevant transactions when
/// presented with multiple conflicting transaction scenarios using the [`TxTemplate`] structure.
/// This test also checks that [`TxGraph::filter_chain_txouts`], [`TxGraph::filter_chain_unspents`],
/// and [`TxGraph::balance`] return correct data.
#[test]
fn test_tx_conflict_handling() {
    // Create Local chains
    let local_chain = local_chain!(
        (0, h!("A")),
        (1, h!("B")),
        (2, h!("C")),
        (3, h!("D")),
        (4, h!("E"))
    );
    let chain_tip = local_chain
        .tip()
        .map(|cp| cp.block_id())
        .unwrap_or_default();

    // Create test transactions

    // Confirmed parent transaction at Block 1.
    let tx1: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx1",
        outputs: &[TxOutTemplate {
            value: 10000,
            spk_index: Some(0),
        }],
        anchors: &[ConfirmationHeightAnchor {
            anchor_block: BlockId {
                height: 1,
                hash: h!("B"),
            },
            confirmation_height: 1,
        }],
        last_seen: None,
        ..Default::default()
    };

    let tx_conflict_1: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx_conflict_1",
        inputs: &[TxInTemplate::PrevTx("tx1", 0)],
        outputs: &[TxOutTemplate {
            value: 20000,
            spk_index: Some(1),
        }],
        last_seen: Some(200),
        ..Default::default()
    };

    let tx_conflict_2: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx_conflict_2",
        inputs: &[TxInTemplate::PrevTx("tx1", 0)],
        outputs: &[TxOutTemplate {
            value: 30000,
            spk_index: Some(2),
        }],
        last_seen: Some(300),
        ..Default::default()
    };

    let tx_conflict_3: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx_conflict_3",
        inputs: &[TxInTemplate::PrevTx("tx1", 0)],
        outputs: &[TxOutTemplate {
            value: 40000,
            spk_index: Some(3),
        }],
        last_seen: Some(400),
        ..Default::default()
    };

    let tx_orphaned_conflict_1: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx_orphaned_conflict_1",
        inputs: &[TxInTemplate::PrevTx("tx1", 0)],
        outputs: &[TxOutTemplate {
            value: 30000,
            spk_index: Some(2),
        }],
        anchors: &[ConfirmationHeightAnchor {
            anchor_block: BlockId {
                height: 5,
                hash: h!("Orphaned Block"),
            },
            confirmation_height: 5,
        }],
        last_seen: Some(300),
    };

    let tx_orphaned_conflict_2: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx_orphaned_conflict_2",
        inputs: &[TxInTemplate::PrevTx("tx1", 0)],
        outputs: &[TxOutTemplate {
            value: 30000,
            spk_index: Some(2),
        }],
        anchors: &[ConfirmationHeightAnchor {
            anchor_block: BlockId {
                height: 5,
                hash: h!("Orphaned Block"),
            },
            confirmation_height: 5,
        }],
        last_seen: Some(100),
    };

    let tx_confirmed_conflict: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
        tx_name: "tx_confirmed_conflict",
        inputs: &[TxInTemplate::PrevTx("tx1", 0)],
        outputs: &[TxOutTemplate {
            value: 50000,
            spk_index: Some(4),
        }],
        anchors: &[ConfirmationHeightAnchor {
            anchor_block: BlockId {
                height: 1,
                hash: h!("B"),
            },
            confirmation_height: 1,
        }],
        ..Default::default()
    };

    // let tx_bestchain_conflict_1: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_bestchain_conflict_1",
    //     inputs: &[TxInTemplate::PrevTx("tx1", 0)],
    //     outputs: &[TxOutTemplate {
    //         value: 20000,
    //         spk_index: Some(1),
    //     }],
    //     ..Default::default()
    // };

    // // Conflicting transaction anchored in best chain
    // let tx_bestchain_conflict_2: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_bestchain_conflict_2",
    //     inputs: &[TxInTemplate::PrevTx("tx1", 0)],
    //     outputs: &[TxOutTemplate {
    //         value: 20000,
    //         spk_index: Some(2),
    //     }],
    //     anchors: &[ConfirmationHeightAnchor {
    //         anchor_block: BlockId {
    //             height: 4,
    //             hash: h!("E"),
    //         },
    //         confirmation_height: 4,
    //     }],
    //     ..Default::default()
    // };

    // let tx_bestchain_spend_1: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_bestchain_spend_1",
    //     inputs: &[TxInTemplate::PrevTx("tx_bestchain_conflict_1", 0)],
    //     outputs: &[TxOutTemplate {
    //         value: 30000,
    //         spk_index: Some(3),
    //     }],
    //     ..Default::default()
    // };

    // let tx_bestchain_spend_2: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_bestchain_spend_2",
    //     inputs: &[TxInTemplate::PrevTx("tx_bestchain_conflict_2", 0)],
    //     outputs: &[TxOutTemplate {
    //         value: 30000,
    //         spk_index: Some(3),
    //     }],
    //     ..Default::default()
    // };

    // let tx_inputs_conflict_1: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_inputs_conflict_1",
    //     inputs: &[
    //         TxInTemplate::PrevTx("tx_conflict_1", 0),
    //         TxInTemplate::PrevTx("tx_conflict_2", 0),
    //     ],
    //     outputs: &[TxOutTemplate {
    //         value: 20000,
    //         spk_index: Some(3),
    //     }],
    //     ..Default::default()
    // };

    // let tx_inputs_conflict_2: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_inputs_conflict_2",
    //     inputs: &[
    //         TxInTemplate::PrevTx("tx_conflict_1", 0),
    //         TxInTemplate::PrevTx("tx_confirmed_conflict", 0),
    //     ],
    //     outputs: &[TxOutTemplate {
    //         value: 20000,
    //         spk_index: Some(5),
    //     }],
    //     ..Default::default()
    // };

    // let tx_spend_inputs_conflict: TxTemplate<'_, ConfirmationHeightAnchor> = TxTemplate {
    //     tx_name: "tx_spend_inputs_conflict",
    //     inputs: &[TxInTemplate::PrevTx("tx_inputs_conflict_2", 0)],
    //     outputs: &[TxOutTemplate {
    //         value: 20000,
    //         spk_index: Some(6),
    //     }],
    //     ..Default::default()
    // };

    let scenarios = [
        Scenario {
            name: "2 unconfirmed txs with different last_seens conflict",
            tx_templates: &[tx1, tx_conflict_1, tx_conflict_2],
            exp_chain_txs: HashSet::from([("tx1", 0), ("tx_conflict_2", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_2", 0)]),
            exp_balance: Balance {
                immature: 0,
                trusted_pending: 30000,
                untrusted_pending: 0,
                confirmed: 0,
            },
        },
        Scenario {
            name: "3 unconfirmed txs with different last_seens conflict",
            tx_templates: &[tx1, tx_conflict_1, tx_conflict_2, tx_conflict_3],
            exp_chain_txs: HashSet::from([("tx1", 0), ("tx_conflict_3", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_3", 0)]),
            exp_balance: Balance {
                immature: 0,
                trusted_pending: 40000,
                untrusted_pending: 0,
                confirmed: 0,
            },
        },
        Scenario {
            name: "unconfirmed tx conflicts with tx in orphaned block, orphaned higher unseen",
            tx_templates: &[tx1, tx_conflict_1, tx_orphaned_conflict_1],
            exp_chain_txs: HashSet::from([("tx1", 0), ("tx_orphaned_conflict_1", 0)]),
            exp_unspents: HashSet::from([("tx_orphaned_conflict_1", 0)]),
            exp_balance: Balance {
                immature: 0,
                trusted_pending: 30000,
                untrusted_pending: 0,
                confirmed: 0,
            },
        },
        Scenario {
            name: "unconfirmed tx conflicts with tx in orphaned block, orphaned lower unseen",
            tx_templates: &[tx1, tx_conflict_1, tx_orphaned_conflict_2],
            exp_chain_txs: HashSet::from([("tx1", 0), ("tx_conflict_1", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_1", 0)]),
            exp_balance: Balance {
                immature: 0,
                trusted_pending: 20000,
                untrusted_pending: 0,
                confirmed: 0,
            },
        },
        Scenario {
            name: "multiple unconfirmed txs conflict with a confirmed tx",
            tx_templates: &[
                tx1,
                tx_conflict_1,
                tx_conflict_2,
                tx_conflict_3,
                tx_confirmed_conflict,
            ],
            exp_chain_txs: HashSet::from([("tx1", 0), ("tx_confirmed_conflict", 0)]),
            exp_unspents: HashSet::from([("tx_confirmed_conflict", 0)]),
            exp_balance: Balance {
                immature: 0,
                trusted_pending: 0,
                untrusted_pending: 0,
                confirmed: 50000,
            },
        },
        // Scenario {
        //     name: "B and B' conflict, C spends B, B' is anchored in best chain",
        //     tx_templates: &[
        //         tx1,
        //         tx_bestchain_conflict_1, // B
        //         tx_bestchain_conflict_2, // B'
        //         tx_bestchain_spend_1,    // C
        //     ],
        //     // B and C should not appear in the list methods
        //     exp_chain_txs: HashSet::from([("tx1", 0), ("tx_bestchain_conflict_2", 0)]),
        //     exp_unspents: HashSet::from([("tx_bestchain_conflict_2", 0)]),
        //     exp_balance: Balance {
        //         immature: 0,
        //         trusted_pending: 0,
        //         untrusted_pending: 0,
        //         confirmed: 20000,
        //     },
        // },
        // Scenario {
        //     name: "B and B' conflict, C spends B', B' is anchored in best chain",
        //     tx_templates: &[
        //         tx1,
        //         tx_bestchain_conflict_1, // B
        //         tx_bestchain_conflict_2, // B'
        //         tx_bestchain_spend_2,    // C
        //     ],
        //     // B should not appear in the list methods
        //     exp_chain_txs: HashSet::from([
        //         ("tx1", 0),
        //         ("tx_bestchain_conflict_2", 0),
        //         ("tx_bestchain_spend_2", 0),
        //     ]),
        //     exp_unspents: HashSet::from([("tx_bestchain_spend_2", 0)]),
        //     exp_balance: Balance {
        //         immature: 0,
        //         trusted_pending: 30000,
        //         untrusted_pending: 0,
        //         confirmed: 0,
        //     },
        // },
        // Scenario {
        //     name: "B and B' conflict, C spends both B and B'",
        //     tx_templates: &[
        //         tx1,
        //         tx_conflict_1,        // B
        //         tx_conflict_2,        // B'
        //         tx_inputs_conflict_1, // C
        //     ],
        //     // C should not appear in the list methods
        //     exp_chain_txs: HashSet::from([("tx1", 0), ("tx_conflict_2", 0)]),
        //     exp_unspents: HashSet::from([("tx_conflict_2", 0)]),
        //     exp_balance: Balance {
        //         immature: 0,
        //         trusted_pending: 30000,
        //         untrusted_pending: 0,
        //         confirmed: 0,
        //     },
        // },
        // Scenario {
        //     name: "B and B' conflict, B' is confirmed, C spends both B and B'",
        //     tx_templates: &[
        //         tx1,
        //         tx_conflict_1,         // B
        //         tx_confirmed_conflict, // B'
        //         tx_inputs_conflict_2,  // C
        //     ],
        //     // C should not appear in the list methods
        //     exp_chain_txs: HashSet::from([("tx1", 0), ("tx_confirmed_conflict", 0)]),
        //     exp_unspents: HashSet::from([("tx_confirmed_conflict", 0)]),
        //     exp_balance: Balance {
        //         immature: 0,
        //         trusted_pending: 0,
        //         untrusted_pending: 0,
        //         confirmed: 50000,
        //     },
        // },
        // Scenario {
        //     name: "B and B' conflict, B' is confirmed, C spends both B and B', D spends C",
        //     tx_templates: &[
        //         tx1,
        //         tx_conflict_1,            // B
        //         tx_confirmed_conflict,    // B'
        //         tx_inputs_conflict_2,     // C
        //         tx_spend_inputs_conflict, // D
        //     ],
        //     // D should not appear in the list methods
        //     exp_chain_txs: HashSet::from([("tx1", 0), ("tx_confirmed_conflict", 0)]),
        //     exp_unspents: HashSet::from([("tx_confirmed_conflict", 0)]),
        //     exp_balance: Balance {
        //         immature: 0,
        //         trusted_pending: 0,
        //         untrusted_pending: 0,
        //         confirmed: 50000,
        //     },
        // },
    ];

    for scenario in scenarios {
        let (tx_graph, spk_index, exp_tx_ids) = init_graph(scenario.tx_templates.iter());

        let exp_txouts = scenario
            .exp_chain_txs
            .iter()
            .map(|(txid, vout)| OutPoint {
                txid: *exp_tx_ids.get(txid).expect("txid must exist"),
                vout: *vout,
            })
            .collect::<BTreeSet<_>>();
        let exp_utxos = scenario
            .exp_unspents
            .iter()
            .map(|(txid, vout)| OutPoint {
                txid: *exp_tx_ids.get(txid).expect("txid must exist"),
                vout: *vout,
            })
            .collect::<BTreeSet<_>>();

        let txouts = tx_graph
            .filter_chain_txouts(
                &local_chain,
                chain_tip,
                spk_index.outpoints().iter().cloned(),
            )
            .collect::<Vec<_>>()
            .iter()
            .map(|(_, full_txout)| full_txout.outpoint)
            .collect::<BTreeSet<_>>();
        let utxos = tx_graph
            .filter_chain_unspents(
                &local_chain,
                chain_tip,
                spk_index.outpoints().iter().cloned(),
            )
            .collect::<Vec<_>>()
            .iter()
            .map(|(_, full_txout)| full_txout.outpoint)
            .collect::<BTreeSet<_>>();
        let balance = tx_graph.balance(
            &local_chain,
            chain_tip,
            spk_index.outpoints().iter().cloned(),
            |_, spk: &Script| spk_index.index_of_spk(spk).is_some(),
        );

        assert_eq!(
            txouts, exp_txouts,
            "\n[{}] 'filter_chain_txouts' failed",
            scenario.name
        );
        assert_eq!(
            utxos, exp_utxos,
            "\n[{}] 'filter_chain_unspents' failed",
            scenario.name
        );
        assert_eq!(
            balance, scenario.exp_balance,
            "\n[{}] 'balance' failed",
            scenario.name
        );
    }
}

#[allow(unused)]
pub fn single_descriptor_setup() -> (
    LocalChain,
    IndexedTxGraph<ConfirmationHeightAnchor, KeychainTxOutIndex<()>>,
    Descriptor<DescriptorPublicKey>,
) {
    let local_chain = (0..10)
        .map(|i| (i as u32, BlockHash::hash(format!("Block {}", i).as_bytes())))
        .collect::<BTreeMap<u32, BlockHash>>();
    let local_chain = LocalChain::from(local_chain);

    let (desc_1, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)").unwrap();

    let mut graph = IndexedTxGraph::<ConfirmationHeightAnchor, KeychainTxOutIndex<()>>::default();

    graph.index.add_keychain((), desc_1.clone());
    graph.index.set_lookahead_for_all(100);

    (local_chain, graph, desc_1)
}

#[allow(unused)]
pub fn setup_conflicts(
    spk_iter: &mut SpkIterator<&Descriptor<DescriptorPublicKey>>,
) -> (Transaction, Transaction, Transaction) {
    let tx1 = Transaction {
        output: vec![TxOut {
            script_pubkey: spk_iter.next().unwrap().1,
            value: 10000,
        }],
        ..new_tx(0)
    };

    let tx_conflict_1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx1.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            script_pubkey: spk_iter.next().unwrap().1,
            value: 20000,
        }],
        ..new_tx(0)
    };

    let tx_conflict_2 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx1.txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            script_pubkey: spk_iter.next().unwrap().1,
            value: 30000,
        }],
        ..new_tx(0)
    };

    (tx1, tx_conflict_1, tx_conflict_2)
}

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

    assert_eq!(
        graph.insert_relevant_txs(txs.iter().map(|tx| (tx, None)), None),
        indexed_tx_graph::ChangeSet {
            graph: tx_graph::ChangeSet {
                txs: txs.into(),
                ..Default::default()
            },
            indexer: keychain::ChangeSet([((), 9_u32)].into()),
        }
    )
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

    // tx6 is an irrelevant transaction confirmed at Block 3.
    let tx6 = common::new_tx(0);

    // Insert transactions into graph with respective anchors
    // For unconfirmed txs we pass in `None`.

    let _ = graph.insert_relevant_txs(
        [&tx1, &tx2, &tx3, &tx6].iter().enumerate().map(|(i, tx)| {
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
            )
        }),
        None,
    );

    let _ = graph.insert_relevant_txs([&tx4, &tx5].iter().map(|tx| (*tx, None)), Some(100));

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

/// Check conflicts between two in mempool transactions. Tx with older `seen_at` is filtered out.
#[test]
fn test_unconfirmed_conflicts() {
    let (local_chain, mut graph, desc) = single_descriptor_setup();
    let mut spk_iter = SpkIterator::new(&desc);
    let (parent_tx, tx_conflict_1, tx_conflict_2) = setup_conflicts(&mut spk_iter);

    // Parent tx is confirmed at height 2.
    let _ = graph.insert_relevant_txs(
        [&parent_tx].iter().map(|tx| {
            (
                *tx,
                [ConfirmationHeightAnchor {
                    anchor_block: (2, *local_chain.blocks().get(&2).unwrap()).into(),
                    confirmation_height: 2,
                }],
            )
        }),
        None,
    );

    // Conflict 1 is seen at 100
    let _ = graph.insert_relevant_txs([&tx_conflict_1].iter().map(|tx| (*tx, None)), Some(100));

    // Conflict 2 is seen at 200
    let _ = graph.insert_relevant_txs([&tx_conflict_2].iter().map(|tx| (*tx, None)), Some(200));

    let txout_confirmations = graph
        .graph()
        .filter_chain_txouts(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .map(|(_, txout)| txout.chain_position)
        .collect::<BTreeSet<_>>();

    let mut utxos = graph
        .graph()
        .filter_chain_unspents(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .collect::<Vec<_>>();

    // We only have 2 txouts. The confirmed `tx_1` and latest `tx_conflict_2`
    assert_eq!(txout_confirmations.len(), 2);
    assert_eq!(
        txout_confirmations,
        [
            ChainPosition::Confirmed(ConfirmationHeightAnchor {
                anchor_block: (2u32, *local_chain.blocks().get(&2).unwrap()).into(),
                confirmation_height: 2
            }),
            ChainPosition::Unconfirmed(200)
        ]
        .into()
    );

    // We only have one utxo. The latest `tx_conflict_2`.
    assert_eq!(
        utxos.pop().unwrap().1.chain_position,
        ChainPosition::Unconfirmed(200)
    );
}

/// Check conflict between mempool and orphaned block. Tx in orphaned block is filtered out.
#[test]
fn test_orphaned_conflicts() {
    let (local_chain, mut graph, desc) = single_descriptor_setup();
    let mut spk_iter = SpkIterator::new(&desc);
    let (parent_tx, tx_conflict_1, tx_conflict_2) = setup_conflicts(&mut spk_iter);

    // Parent tx confirmed at height 2.
    let _ = graph.insert_relevant_txs(
        [&parent_tx].iter().map(|tx| {
            (
                *tx,
                [ConfirmationHeightAnchor {
                    anchor_block: (2, *local_chain.blocks().get(&2).unwrap()).into(),
                    confirmation_height: 2,
                }],
            )
        }),
        None,
    );

    // Ophaned block at height 5.
    let orphaned_block = BlockId {
        hash: h!("Orphaned Block"),
        height: 5,
    };

    // 1st conflicting tx is in mempool.
    let _ = graph.insert_relevant_txs([&tx_conflict_1].iter().map(|tx| (*tx, None)), Some(100));

    // Second conflicting tx is in orphaned block.
    let _ = graph.insert_relevant_txs(
        [&tx_conflict_2].iter().map(|tx| {
            (
                *tx,
                [ConfirmationHeightAnchor {
                    anchor_block: orphaned_block,
                    confirmation_height: 5,
                }],
            )
        }),
        None,
    );

    let txout_confirmations = graph
        .graph()
        .filter_chain_txouts(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .map(|(_, txout)| txout.chain_position)
        .collect::<BTreeSet<_>>();

    let mut utxos = graph
        .graph()
        .filter_chain_unspents(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .collect::<Vec<_>>();

    // We only have the mempool tx. Conflicting orphaned is ignored.
    assert_eq!(txout_confirmations.len(), 2);
    assert_eq!(
        txout_confirmations,
        [
            ChainPosition::Confirmed(ConfirmationHeightAnchor {
                anchor_block: (2u32, *local_chain.blocks().get(&2).unwrap()).into(),
                confirmation_height: 2
            }),
            ChainPosition::Unconfirmed(100)
        ]
        .into()
    );

    // We only have one utxo and its in mempool.
    assert_eq!(
        utxos.pop().unwrap().1.chain_position,
        ChainPosition::Unconfirmed(100)
    );
}

/// Check conflicts between mempool and confirmed tx. Mempool tx is filtered out.
#[test]
fn test_confirmed_conflicts() {
    let (local_chain, mut graph, desc) = single_descriptor_setup();
    let mut spk_iter = SpkIterator::new(&desc);
    let (parent_tx, tx_conflict_1, tx_conflict_2) = setup_conflicts(&mut spk_iter);

    // Parent confirms at height 2.
    let _ = graph.insert_relevant_txs(
        [&parent_tx].iter().map(|tx| {
            (
                *tx,
                [ConfirmationHeightAnchor {
                    anchor_block: (2, *local_chain.blocks().get(&2).unwrap()).into(),
                    confirmation_height: 2,
                }],
            )
        }),
        None,
    );

    // `tx_conflict_1` is in mempool.
    let _ = graph.insert_relevant_txs([&tx_conflict_1].iter().map(|tx| (*tx, None)), Some(100));

    // `tx_conflict_2` is in orphaned block at height 5.
    let _ = graph.insert_relevant_txs(
        [&tx_conflict_2].iter().map(|tx| {
            (
                *tx,
                [ConfirmationHeightAnchor {
                    anchor_block: (2, *local_chain.blocks().get(&2).unwrap()).into(),
                    confirmation_height: 2,
                }],
            )
        }),
        None,
    );

    let txout_confirmations = graph
        .graph()
        .filter_chain_txouts(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .map(|(_, txout)| txout.chain_position)
        .collect::<BTreeSet<_>>();

    let mut utxos = graph
        .graph()
        .filter_chain_unspents(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .collect::<Vec<_>>();

    // We only have 1 txout. Confirmed at block 2.
    assert_eq!(txout_confirmations.len(), 1);
    assert_eq!(
        txout_confirmations,
        [ChainPosition::Confirmed(ConfirmationHeightAnchor {
            anchor_block: (2, *local_chain.blocks().get(&2).unwrap()).into(),
            confirmation_height: 2
        })]
        .into()
    );

    // We only have one utxo, confirmed at block 2.
    assert_eq!(
        utxos.pop().unwrap().1.chain_position,
        ChainPosition::Confirmed(ConfirmationHeightAnchor {
            anchor_block: (2u32, *local_chain.blocks().get(&2).unwrap()).into(),
            confirmation_height: 2
        }),
    );
}

/// Test conflicts for two mempool tx, with same `seen_at` time.
#[test]
fn test_unconfirmed_conflicts_at_same_last_seen() {
    let (local_chain, mut graph, desc) = single_descriptor_setup();
    let mut spk_iter = SpkIterator::new(&desc);
    let (parent_tx, tx_conflict_1, tx_conflict_2) = setup_conflicts(&mut spk_iter);

    // Parent confirms at height 2.
    let _ = graph.insert_relevant_txs(
        [&parent_tx].iter().map(|tx| {
            (
                *tx,
                [ConfirmationHeightAnchor {
                    anchor_block: (2, *local_chain.blocks().get(&2).unwrap()).into(),
                    confirmation_height: 2,
                }],
            )
        }),
        None,
    );

    // Both conflicts are in mempool at same `seen_at`
    let _ = graph.insert_relevant_txs(
        [&tx_conflict_1, &tx_conflict_2]
            .iter()
            .map(|tx| (*tx, None)),
        Some(100),
    );

    let txouts = graph
        .graph()
        .filter_chain_txouts(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .collect::<Vec<_>>();

    let utxos = graph
        .graph()
        .filter_chain_unspents(
            &local_chain,
            local_chain.tip().unwrap().block_id(),
            graph.index.outpoints().iter().cloned(),
        )
        .collect::<Vec<_>>();

    // FIXME: Currently both the mempool tx are indexed and listed out. This can happen in case of RBF fee bumps,
    // when both the txs are observed at a single sync time. This can be resolved by checking the input's nSequence.
    // Additionally in case of non RBF conflicts at same `seen_at`, conflicting txids can be reported back for filtering
    // out in higher layers. This is similar to what core rpc does in case of unresolvable conflicts.

    // We have two in mempool txouts. Both at same chain position.
    assert_eq!(txouts.len(), 3);
    assert_eq!(
        txouts
            .iter()
            .filter(|(_, txout)| matches!(txout.chain_position, ChainPosition::Unconfirmed(100)))
            .count(),
        2
    );

    // We have two mempool utxos both at same chain position.
    assert_eq!(
        utxos
            .iter()
            .filter(|(_, txout)| matches!(txout.chain_position, ChainPosition::Unconfirmed(100)))
            .count(),
        2
    );
}
