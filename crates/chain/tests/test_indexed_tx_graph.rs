#[macro_use]
mod common;

use std::collections::{BTreeMap, BTreeSet};

use bdk_chain::{
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{Balance, DerivationAdditions, KeychainTxOutIndex},
    tx_graph::Additions,
    BlockId, ObservedAs,
};
use bitcoin::{secp256k1::Secp256k1, BlockHash, OutPoint, Script, Transaction, TxIn, TxOut};
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

#[test]
fn test_list_owned_txouts() {
    let mut local_chain = local_chain![
        (0, h!("Block 0")),
        (1, h!("Block 1")),
        (2, h!("Block 2")),
        (3, h!("Block 3"))
    ];

    let desc_1 : &str = "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/0/*)";
    let (desc_1, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), desc_1).unwrap();
    let desc_2 : &str = "tr(tprv8ZgxMBicQKsPd3krDUsBAmtnRsK3rb8u5yi1zhQgMhF1tR8MW7xfE4rnrbbsrbPR52e7rKapu6ztw1jXveJSCGHEriUGZV7mCe88duLp5pj/86'/1'/0'/1/*)";
    let (desc_2, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), desc_2).unwrap();

    let mut graph = IndexedTxGraph::<BlockId, KeychainTxOutIndex<String>>::default();

    graph.index.add_keychain("keychain_1".into(), desc_1);
    graph.index.add_keychain("keychain_2".into(), desc_2);

    graph.index.set_lookahead_for_all(10);

    let mut trusted_spks = Vec::new();
    let mut untrusted_spks = Vec::new();

    {
        for _ in 0..10 {
            let ((_, script), _) = graph.index.reveal_next_spk(&"keychain_1".to_string());
            trusted_spks.push(script.clone());
        }
    }

    {
        for _ in 0..10 {
            let ((_, script), _) = graph.index.reveal_next_spk(&"keychain_2".to_string());
            untrusted_spks.push(script.clone());
        }
    }

    let trust_predicate = |spk: &Script| trusted_spks.contains(&spk);

    // tx1 is coinbase transaction received at trusted keychain at block 0.
    let tx1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 70000,
            script_pubkey: trusted_spks[0].clone(),
        }],
        ..common::new_tx(0)
    };

    // tx2 is an incoming transaction received at untrusted keychain at block 1.
    let tx2 = Transaction {
        output: vec![TxOut {
            value: 30000,
            script_pubkey: untrusted_spks[0].clone(),
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
            script_pubkey: trusted_spks[1].clone(),
        }],
        ..common::new_tx(0)
    };

    // tx4 is an external transaction receiving at untrusted keychain, unconfirmed.
    let tx4 = Transaction {
        output: vec![TxOut {
            value: 20000,
            script_pubkey: untrusted_spks[1].clone(),
        }],
        ..common::new_tx(0)
    };

    // tx5 is spending tx3 and receiving change at trusted keychain, unconfirmed.
    let tx5 = Transaction {
        output: vec![TxOut {
            value: 15000,
            script_pubkey: trusted_spks[2].clone(),
        }],
        ..common::new_tx(0)
    };

    // tx6 is an unrelated transaction confirmed at 3.
    let tx6 = common::new_tx(0);

    let _ = graph.insert_relevant_txs(
        [&tx1, &tx2, &tx3, &tx6]
            .iter()
            .enumerate()
            .map(|(i, tx)| (*tx, [local_chain.get_block(i as u32).unwrap()])),
        None,
    );

    let _ = graph.insert_relevant_txs([&tx4, &tx5].iter().map(|tx| (*tx, None)), Some(100));

    // AT Block 0
    {
        let txouts = graph
            .list_owned_txouts(&local_chain, local_chain.get_block(0).unwrap())
            .collect::<Vec<_>>();

        let utxos = graph
            .list_owned_unspents(&local_chain, local_chain.get_block(0).unwrap())
            .collect::<Vec<_>>();

        let balance = graph.balance(
            &local_chain,
            local_chain.get_block(0).unwrap(),
            trust_predicate,
        );

        let confirmed_txouts_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_txout_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let confirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(txouts.iter().count(), 5);
        assert_eq!(utxos.iter().count(), 4);

        assert_eq!(confirmed_txouts_txid, [tx1.txid()].into());
        assert_eq!(
            unconfirmed_txout_txid,
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
        let txouts = graph
            .list_owned_txouts(&local_chain, local_chain.get_block(1).unwrap())
            .collect::<Vec<_>>();

        let utxos = graph
            .list_owned_unspents(&local_chain, local_chain.get_block(1).unwrap())
            .collect::<Vec<_>>();

        let balance = graph.balance(
            &local_chain,
            local_chain.get_block(1).unwrap(),
            trust_predicate,
        );

        let confirmed_txouts_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_txout_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let confirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(txouts.iter().count(), 5);
        assert_eq!(utxos.iter().count(), 4);

        // tx2 gets into confirmed txout set
        assert_eq!(confirmed_txouts_txid, [tx1.txid(), tx2.txid()].into());
        assert_eq!(
            unconfirmed_txout_txid,
            [tx3.txid(), tx4.txid(), tx5.txid()].into()
        );

        // tx2 doesn't get into confirmed utxos set
        assert_eq!(confirmed_utxos_txid, [tx1.txid()].into());
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx3.txid(), tx4.txid(), tx5.txid()].into()
        );

        // Balance breakup remains same
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
        let txouts = graph
            .list_owned_txouts(&local_chain, local_chain.get_block(2).unwrap())
            .collect::<Vec<_>>();

        let utxos = graph
            .list_owned_unspents(&local_chain, local_chain.get_block(2).unwrap())
            .collect::<Vec<_>>();

        let balance = graph.balance(
            &local_chain,
            local_chain.get_block(2).unwrap(),
            trust_predicate,
        );

        let confirmed_txouts_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_txout_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let confirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(txouts.iter().count(), 5);
        assert_eq!(utxos.iter().count(), 4);

        // tx3 now gets into the confirmed txout set
        assert_eq!(
            confirmed_txouts_txid,
            [tx1.txid(), tx2.txid(), tx3.txid()].into()
        );
        assert_eq!(unconfirmed_txout_txid, [tx4.txid(), tx5.txid()].into());

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

    // AT Block 110
    {
        let mut local_chain_extension = (4..150)
            .map(|i| (i as u32, h!("random")))
            .collect::<BTreeMap<u32, BlockHash>>();

        local_chain_extension.insert(3, h!("Block 3"));

        local_chain
            .apply_update(local_chain_extension.into())
            .unwrap();

        let txouts = graph
            .list_owned_txouts(&local_chain, local_chain.get_block(110).unwrap())
            .collect::<Vec<_>>();

        let utxos = graph
            .list_owned_unspents(&local_chain, local_chain.get_block(110).unwrap())
            .collect::<Vec<_>>();

        let balance = graph.balance(
            &local_chain,
            local_chain.get_block(110).unwrap(),
            trust_predicate,
        );

        let confirmed_txouts_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_txout_txid = txouts
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let confirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Confirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        let unconfirmed_utxos_txid = utxos
            .iter()
            .filter_map(|full_txout| {
                if matches!(full_txout.chain_position, ObservedAs::Unconfirmed(_)) {
                    Some(full_txout.outpoint.txid)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();

        println!("TxOuts : {:#?}", txouts);
        println!("UTXOS {:#?}", utxos);
        println!("{:#?}", balance);

        assert_eq!(txouts.iter().count(), 5);
        assert_eq!(utxos.iter().count(), 4);

        assert_eq!(
            confirmed_txouts_txid,
            [tx1.txid(), tx2.txid(), tx3.txid()].into()
        );
        assert_eq!(unconfirmed_txout_txid, [tx4.txid(), tx5.txid()].into());

        assert_eq!(confirmed_utxos_txid, [tx1.txid(), tx3.txid()].into());
        assert_eq!(unconfirmed_utxos_txid, [tx4.txid(), tx5.txid()].into());

        assert_eq!(
            balance,
            Balance {
                immature: 0,              // immature coinbase
                trusted_pending: 15000,   // tx5
                untrusted_pending: 20000, // tx4
                confirmed: 80000          // tx1 got matured
            }
        );
    }
}
