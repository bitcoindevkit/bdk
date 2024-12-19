#![cfg(feature = "miniscript")]

#[macro_use]
mod common;

use std::{collections::BTreeSet, sync::Arc};

use bdk_chain::{
    indexed_tx_graph::{self, IndexedTxGraph},
    indexer::keychain_txout::KeychainTxOutIndex,
    local_chain::LocalChain,
    tx_graph, Balance, ChainPosition, ConfirmationBlockTime, DescriptorExt, Indexer,
};
use bdk_testenv::{
    block_id, hash,
    utils::{new_tx, DESCRIPTORS},
};
use bitcoin::{secp256k1::Secp256k1, Amount, OutPoint, ScriptBuf, Transaction, TxIn, TxOut};
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
    use bdk_chain::indexer::keychain_txout;
    let (descriptor, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[0])
        .expect("must be valid");
    let spk_0 = descriptor.at_derivation_index(0).unwrap().script_pubkey();
    let spk_1 = descriptor.at_derivation_index(9).unwrap().script_pubkey();

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<()>>::new(
        KeychainTxOutIndex::new(10),
    );
    let _ = graph
        .index
        .insert_descriptor((), descriptor.clone())
        .unwrap();

    let tx_a = Transaction {
        output: vec![
            TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: spk_0,
            },
            TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: spk_1,
            },
        ],
        ..new_tx(0)
    };

    let tx_b = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.compute_txid(), 0),
            ..Default::default()
        }],
        ..new_tx(1)
    };

    let tx_c = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx_a.compute_txid(), 1),
            ..Default::default()
        }],
        ..new_tx(2)
    };

    let txs = [tx_c, tx_b, tx_a];

    let changeset = indexed_tx_graph::ChangeSet {
        tx_graph: tx_graph::ChangeSet {
            txs: txs.iter().cloned().map(Arc::new).collect(),
            ..Default::default()
        },
        indexer: keychain_txout::ChangeSet {
            last_revealed: [(descriptor.descriptor_id(), 9_u32)].into(),
        },
    };

    assert_eq!(
        graph.batch_insert_relevant(txs.iter().cloned().map(|tx| (tx, None))),
        changeset,
    );

    // The initial changeset will also contain info about the keychain we added
    let initial_changeset = indexed_tx_graph::ChangeSet {
        tx_graph: changeset.tx_graph,
        indexer: keychain_txout::ChangeSet {
            last_revealed: changeset.indexer.last_revealed,
        },
    };

    assert_eq!(graph.initial_changeset(), initial_changeset);
}

/// Ensure that [`IndexedTxGraph::batch_insert_relevant`] adds transactions that are direct
/// conflicts with transactions in our graph but are not directly relevant to it.
///
/// The graph contains three transactions (A, B, and conflicting_tx), with A and B being relevant
/// because B spends from A. This test verifies that `conflicting_tx` is inserted into the graph
/// solely because it is directly conflicting with B.
#[test]
fn insert_relevant_conflicting_txs() {
    use bdk_chain::indexer::keychain_txout;
    let (descriptor, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[0])
        .expect("must be valid");
    let spk_0 = descriptor.at_derivation_index(0).unwrap().script_pubkey();
    let spk_1 = descriptor.at_derivation_index(9).unwrap().script_pubkey();

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<()>>::new(
        KeychainTxOutIndex::new(10),
    );
    let _ = graph
        .index
        .insert_descriptor((), descriptor.clone())
        .unwrap();

    let conflicting_prev_tx = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: spk_0,
        }],
        ..new_tx(0)
    };

    let tx_a = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: spk_1,
        }],
        ..new_tx(1)
    };

    let tx_b = Transaction {
        input: vec![
            TxIn {
                previous_output: OutPoint::new(conflicting_prev_tx.compute_txid(), 0),
                ..Default::default()
            },
            TxIn {
                previous_output: OutPoint::new(tx_a.compute_txid(), 0),
                ..Default::default()
            },
        ],
        ..new_tx(2)
    };

    let conflicting_tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(conflicting_prev_tx.compute_txid(), 0),
            ..Default::default()
        }],
        ..new_tx(3)
    };

    // We add only `tx_a`, `tx_b`, and `conflicting_tx` into the graph. `tx_a` and `tx_b` are
    // relevant to our graph, because `tx_b` spends from `tx_a`. `conflicting_tx` is not directly
    // relevant to our graph because it does not spend from `tx_a`. However, `tx_b` and
    // `conflicting_tx` conflict, because they both spend from the same output of
    // `conflicting_prev_tx`.
    let txs = [tx_a, tx_b, conflicting_tx.clone()];

    let changeset = graph.batch_insert_relevant(txs.iter().cloned().map(|tx| (tx, None)));

    // Confirm that `conflicting_tx` is added to `ChangeSet` only due to it being a conflicting
    // transaction, and not because it is directly relevant to our graph.
    assert!(!graph.index.is_tx_relevant(&conflicting_tx));
    assert!(graph
        .graph()
        .direct_conflicts(&conflicting_tx)
        .next()
        .is_some());

    let expected_changeset = indexed_tx_graph::ChangeSet {
        tx_graph: tx_graph::ChangeSet::<ConfirmationBlockTime> {
            txs: txs.iter().cloned().map(Arc::new).collect(),
            ..Default::default()
        },
        indexer: keychain_txout::ChangeSet {
            last_revealed: [(descriptor.descriptor_id(), 9_u32)].into(),
        },
    };

    assert_eq!(changeset, expected_changeset);
}

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
/// tx4: Mempool tx, sending 20000 sats to "untrusted" address.
/// tx5: Mempool tx, sending 15000 sats to "trusted" address.
/// tx6: Complete unrelated tx. [Block 3]
///
/// Different transactions are added via `insert_relevant_txs`.
/// `list_owned_txout`, `list_owned_utxos` and `balance` method is asserted
/// with expected values at Block height 0, 1, and 2.
///
/// Finally Add more blocks to local chain until tx1 coinbase maturity hits.
/// Assert maturity at coinbase maturity inflection height. Block height 98 and 99.
#[test]
fn test_list_owned_txouts() {
    // Create Local chains
    let local_chain =
        LocalChain::from_blocks((0..150).map(|i| (i as u32, hash!("random"))).collect())
            .expect("must have genesis hash");

    // Initiate IndexedTxGraph

    let (desc_1, _) =
        Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[2]).unwrap();
    let (desc_2, _) =
        Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[3]).unwrap();

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<String>>::new(
        KeychainTxOutIndex::new(10),
    );

    assert!(graph
        .index
        .insert_descriptor("keychain_1".into(), desc_1)
        .unwrap());
    assert!(graph
        .index
        .insert_descriptor("keychain_2".into(), desc_2)
        .unwrap());

    // Get trusted and untrusted addresses

    let mut trusted_spks: Vec<ScriptBuf> = Vec::new();
    let mut untrusted_spks: Vec<ScriptBuf> = Vec::new();

    {
        // we need to scope here to take immutable reference of the graph
        for _ in 0..10 {
            let ((_, script), _) = graph
                .index
                .reveal_next_spk("keychain_1".to_string())
                .unwrap();
            // TODO Assert indexes
            trusted_spks.push(script.to_owned());
        }
    }
    {
        for _ in 0..10 {
            let ((_, script), _) = graph
                .index
                .reveal_next_spk("keychain_2".to_string())
                .unwrap();
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
            value: Amount::from_sat(70000),
            script_pubkey: trusted_spks[0].to_owned(),
        }],
        ..new_tx(1)
    };

    // tx2 is an incoming transaction received at untrusted keychain at block 1.
    let tx2 = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(30000),
            script_pubkey: untrusted_spks[0].to_owned(),
        }],
        ..new_tx(2)
    };

    // tx3 spends tx2 and gives a change back in trusted keychain. Confirmed at Block 2.
    let tx3 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(tx2.compute_txid(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(10000),
            script_pubkey: trusted_spks[1].to_owned(),
        }],
        ..new_tx(3)
    };

    // tx4 is an external transaction receiving at untrusted keychain, unconfirmed.
    let tx4 = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(20000),
            script_pubkey: untrusted_spks[1].to_owned(),
        }],
        ..new_tx(4)
    };

    // tx5 is an external transaction receiving at trusted keychain, unconfirmed.
    let tx5 = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(15000),
            script_pubkey: trusted_spks[2].to_owned(),
        }],
        ..new_tx(5)
    };

    // tx6 is an unrelated transaction confirmed at 3.
    // This won't be inserted because it is not relevant.
    let tx6 = new_tx(6);

    // Insert transactions into graph with respective anchors
    // Insert unconfirmed txs with a last_seen timestamp

    let _ =
        graph.batch_insert_relevant([&tx1, &tx2, &tx3, &tx6].iter().enumerate().map(|(i, &tx)| {
            let height = i as u32;
            (
                tx.clone(),
                local_chain
                    .get(height)
                    .map(|cp| cp.block_id())
                    .map(|block_id| ConfirmationBlockTime {
                        block_id,
                        confirmation_time: 100,
                    }),
            )
        }));

    let _ =
        graph.batch_insert_relevant_unconfirmed([&tx4, &tx5].iter().map(|&tx| (tx.clone(), 100)));

    // A helper lambda to extract and filter data from the graph.
    let fetch =
        |height: u32, graph: &IndexedTxGraph<ConfirmationBlockTime, KeychainTxOutIndex<String>>| {
            let chain_tip = local_chain
                .get(height)
                .map(|cp| cp.block_id())
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
                |_, spk: ScriptBuf| trusted_spks.contains(&spk),
            );

            let confirmed_txouts_txid = txouts
                .iter()
                .filter_map(|(_, full_txout)| {
                    if full_txout.chain_position.is_confirmed() {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            let unconfirmed_txouts_txid = txouts
                .iter()
                .filter_map(|(_, full_txout)| {
                    if !full_txout.chain_position.is_confirmed() {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            let confirmed_utxos_txid = utxos
                .iter()
                .filter_map(|(_, full_txout)| {
                    if full_txout.chain_position.is_confirmed() {
                        Some(full_txout.outpoint.txid)
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>();

            let unconfirmed_utxos_txid = utxos
                .iter()
                .filter_map(|(_, full_txout)| {
                    if !full_txout.chain_position.is_confirmed() {
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

        // tx1 is a confirmed txout and is unspent
        // tx4, tx5 are unconfirmed
        assert_eq!(confirmed_txouts_txid, [tx1.compute_txid()].into());
        assert_eq!(
            unconfirmed_txouts_txid,
            [
                tx2.compute_txid(),
                tx3.compute_txid(),
                tx4.compute_txid(),
                tx5.compute_txid()
            ]
            .into()
        );

        assert_eq!(confirmed_utxos_txid, [tx1.compute_txid()].into());
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx3.compute_txid(), tx4.compute_txid(), tx5.compute_txid()].into()
        );

        assert_eq!(
            balance,
            Balance {
                immature: Amount::from_sat(70000),          // immature coinbase
                trusted_pending: Amount::from_sat(25000),   // tx3, tx5
                untrusted_pending: Amount::from_sat(20000), // tx4
                confirmed: Amount::ZERO                     // Nothing is confirmed yet
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
        assert_eq!(
            confirmed_txouts_txid,
            [tx1.compute_txid(), tx2.compute_txid()].into()
        );
        assert_eq!(
            unconfirmed_txouts_txid,
            [tx3.compute_txid(), tx4.compute_txid(), tx5.compute_txid()].into()
        );

        // tx2 gets into confirmed utxos set
        assert_eq!(confirmed_utxos_txid, [tx1.compute_txid()].into());
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx3.compute_txid(), tx4.compute_txid(), tx5.compute_txid()].into()
        );

        assert_eq!(
            balance,
            Balance {
                immature: Amount::from_sat(70000),          // immature coinbase
                trusted_pending: Amount::from_sat(25000),   // tx3, tx5
                untrusted_pending: Amount::from_sat(20000), // tx4
                confirmed: Amount::from_sat(0)              // tx2 got confirmed (but spent by 3)
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
            [tx1.compute_txid(), tx2.compute_txid(), tx3.compute_txid()].into()
        );
        assert_eq!(
            unconfirmed_txouts_txid,
            [tx4.compute_txid(), tx5.compute_txid()].into()
        );

        // tx3 also gets into confirmed utxo set
        assert_eq!(
            confirmed_utxos_txid,
            [tx1.compute_txid(), tx3.compute_txid()].into()
        );
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx4.compute_txid(), tx5.compute_txid()].into()
        );

        assert_eq!(
            balance,
            Balance {
                immature: Amount::from_sat(70000),          // immature coinbase
                trusted_pending: Amount::from_sat(15000),   // tx5
                untrusted_pending: Amount::from_sat(20000), // tx4
                confirmed: Amount::from_sat(10000)          // tx3 got confirmed
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

        // no change compared to block 2
        assert_eq!(
            confirmed_txouts_txid,
            [tx1.compute_txid(), tx2.compute_txid(), tx3.compute_txid()].into()
        );
        assert_eq!(
            unconfirmed_txouts_txid,
            [tx4.compute_txid(), tx5.compute_txid()].into()
        );

        assert_eq!(
            confirmed_utxos_txid,
            [tx1.compute_txid(), tx3.compute_txid()].into()
        );
        assert_eq!(
            unconfirmed_utxos_txid,
            [tx4.compute_txid(), tx5.compute_txid()].into()
        );

        // Coinbase is still immature
        assert_eq!(
            balance,
            Balance {
                immature: Amount::from_sat(70000),          // immature coinbase
                trusted_pending: Amount::from_sat(15000),   // tx5
                untrusted_pending: Amount::from_sat(20000), // tx4
                confirmed: Amount::from_sat(10000)          // tx3 is confirmed
            }
        );
    }

    // AT Block 99
    {
        let (_, _, _, _, balance) = fetch(99, &graph);

        // Coinbase maturity hits
        assert_eq!(
            balance,
            Balance {
                immature: Amount::ZERO,                     // coinbase matured
                trusted_pending: Amount::from_sat(15000),   // tx5
                untrusted_pending: Amount::from_sat(20000), // tx4
                confirmed: Amount::from_sat(80000)          // tx1 + tx3
            }
        );
    }
}

/// Given a `LocalChain`, `IndexedTxGraph`, and a `Transaction`, when we insert some anchor
/// (possibly non-canonical) and/or a last-seen timestamp into the graph, we check the canonical
/// position of the tx:
///
/// - tx with no anchors or last_seen has no `ChainPosition`
/// - tx with any last_seen will be `Unconfirmed`
/// - tx with an anchor in best chain will be `Confirmed`
/// - tx with an anchor not in best chain (no last_seen) has no `ChainPosition`
#[test]
fn test_get_chain_position() {
    use bdk_chain::local_chain::CheckPoint;
    use bdk_chain::spk_txout::SpkTxOutIndex;
    use bdk_chain::BlockId;

    #[derive(Debug)]
    struct TestCase<A> {
        name: &'static str,
        tx: Transaction,
        anchor: Option<A>,
        last_seen: Option<u64>,
        exp_pos: Option<ChainPosition<A>>,
    }

    // addr: bcrt1qc6fweuf4xjvz4x3gx3t9e0fh4hvqyu2qw4wvxm
    let spk = ScriptBuf::from_hex("0014c692ecf13534982a9a2834565cbd37add8027140").unwrap();
    let mut graph = IndexedTxGraph::new({
        let mut index = SpkTxOutIndex::default();
        let _ = index.insert_spk(0u32, spk.clone());
        index
    });

    // Anchors to test
    let blocks = vec![block_id!(0, "g"), block_id!(1, "A"), block_id!(2, "B")];

    let cp = CheckPoint::from_block_ids(blocks.clone()).unwrap();
    let chain = LocalChain::from_tip(cp).unwrap();

    // The test will insert a transaction into the indexed tx graph along with any anchors and
    // timestamps, then check the tx's canonical position is expected.
    fn run(
        chain: &LocalChain,
        graph: &mut IndexedTxGraph<BlockId, SpkTxOutIndex<u32>>,
        test: TestCase<BlockId>,
    ) {
        let TestCase {
            name,
            tx,
            anchor,
            last_seen,
            exp_pos,
        } = test;

        // add data to graph
        let txid = tx.compute_txid();
        let _ = graph.insert_tx(tx);
        if let Some(anchor) = anchor {
            let _ = graph.insert_anchor(txid, anchor);
        }
        if let Some(seen_at) = last_seen {
            let _ = graph.insert_seen_at(txid, seen_at);
        }

        // check chain position
        let chain_pos = graph
            .graph()
            .list_canonical_txs(chain, chain.tip().block_id())
            .find_map(|canon_tx| {
                if canon_tx.tx_node.txid == txid {
                    Some(canon_tx.chain_position)
                } else {
                    None
                }
            });
        assert_eq!(chain_pos, exp_pos, "failed test case: {name}");
    }

    [
        TestCase {
            name: "tx no anchors or last_seen - no chain pos",
            tx: Transaction {
                output: vec![TxOut {
                    value: Amount::ONE_BTC,
                    script_pubkey: spk.clone(),
                }],
                ..new_tx(0)
            },
            anchor: None,
            last_seen: None,
            exp_pos: None,
        },
        TestCase {
            name: "tx last_seen - unconfirmed",
            tx: Transaction {
                output: vec![TxOut {
                    value: Amount::ONE_BTC,
                    script_pubkey: spk.clone(),
                }],
                ..new_tx(1)
            },
            anchor: None,
            last_seen: Some(2),
            exp_pos: Some(ChainPosition::Unconfirmed { last_seen: Some(2) }),
        },
        TestCase {
            name: "tx anchor in best chain - confirmed",
            tx: Transaction {
                output: vec![TxOut {
                    value: Amount::ONE_BTC,
                    script_pubkey: spk.clone(),
                }],
                ..new_tx(2)
            },
            anchor: Some(blocks[1]),
            last_seen: None,
            exp_pos: Some(ChainPosition::Confirmed {
                anchor: blocks[1],
                transitively: None,
            }),
        },
        TestCase {
            name: "tx unknown anchor with last_seen - unconfirmed",
            tx: Transaction {
                output: vec![TxOut {
                    value: Amount::ONE_BTC,
                    script_pubkey: spk.clone(),
                }],
                ..new_tx(3)
            },
            anchor: Some(block_id!(2, "B'")),
            last_seen: Some(2),
            exp_pos: Some(ChainPosition::Unconfirmed { last_seen: Some(2) }),
        },
        TestCase {
            name: "tx unknown anchor - unconfirmed",
            tx: Transaction {
                output: vec![TxOut {
                    value: Amount::ONE_BTC,
                    script_pubkey: spk.clone(),
                }],
                ..new_tx(4)
            },
            anchor: Some(block_id!(2, "B'")),
            last_seen: None,
            exp_pos: Some(ChainPosition::Unconfirmed { last_seen: None }),
        },
    ]
    .into_iter()
    .for_each(|t| run(&chain, &mut graph, t));
}
