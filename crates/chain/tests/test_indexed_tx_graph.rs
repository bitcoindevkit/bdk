#![cfg(feature = "miniscript")]

#[macro_use]
mod common;

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use bdk_chain::{
    indexed_tx_graph::{self, IndexedTxGraph},
    indexer::keychain_txout::KeychainTxOutIndex,
    local_chain::LocalChain,
    spk_txout::SpkTxOutIndex,
    tx_graph, Balance, CanonicalizationParams, ChainPosition, ConfirmationBlockTime, DescriptorExt,
    SpkIterator,
};
use bdk_testenv::{
    anyhow::{self},
    bitcoincore_rpc::{json::CreateRawTransactionInput, RpcApi},
    block_id, hash,
    utils::{new_tx, DESCRIPTORS},
    TestEnv,
};
use bitcoin::{
    secp256k1::Secp256k1, Address, Amount, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut,
    Txid,
};
use miniscript::Descriptor;

fn gen_spk() -> ScriptBuf {
    use bitcoin::secp256k1::{Secp256k1, SecretKey};

    let secp = Secp256k1::new();
    let (x_only_pk, _) = SecretKey::new(&mut rand::thread_rng())
        .public_key(&secp)
        .x_only_public_key();
    ScriptBuf::new_p2tr(&secp, x_only_pk, None)
}

/// Conflicts of relevant transactions must also be considered relevant.
///
/// This allows the receiving structures to determine the reason why a given transaction is not part
/// of the best history. I.e. Is this transaction evicted from the mempool because of insufficient
/// fee, or because a conflict is confirmed?
///
/// This tests the behavior of the "relevant-conflicts" logic.
#[test]
fn relevant_conflicts() -> anyhow::Result<()> {
    type SpkTxGraph = IndexedTxGraph<ConfirmationBlockTime, SpkTxOutIndex<()>>;

    /// This environment contains a sender and receiver.
    ///
    /// The sender sends a transaction to the receiver and attempts to cancel it later.
    struct ScenarioEnv {
        env: TestEnv,
        graph: SpkTxGraph,
        tx_send: Transaction,
        tx_cancel: Transaction,
    }

    impl ScenarioEnv {
        fn new() -> anyhow::Result<Self> {
            let env = TestEnv::new()?;
            let client = env.rpc_client();

            let sender_addr = client
                .get_new_address(None, None)?
                .require_network(Network::Regtest)?;

            let recv_spk = gen_spk();
            let recv_addr = Address::from_script(&recv_spk, &bitcoin::params::REGTEST)?;

            let mut graph = SpkTxGraph::default();
            assert!(graph.index.insert_spk((), recv_spk));

            env.mine_blocks(1, Some(sender_addr.clone()))?;
            env.mine_blocks(101, None)?;

            let tx_input = client
                .list_unspent(None, None, None, None, None)?
                .into_iter()
                .take(1)
                .map(|r| CreateRawTransactionInput {
                    txid: r.txid,
                    vout: r.vout,
                    sequence: None,
                })
                .collect::<Vec<_>>();
            let tx_send = {
                let outputs =
                    HashMap::from([(recv_addr.to_string(), Amount::from_btc(49.999_99)?)]);
                let tx = client.create_raw_transaction(&tx_input, &outputs, None, Some(true))?;
                client
                    .sign_raw_transaction_with_wallet(&tx, None, None)?
                    .transaction()?
            };
            let tx_cancel = {
                let outputs =
                    HashMap::from([(sender_addr.to_string(), Amount::from_btc(49.999_98)?)]);
                let tx = client.create_raw_transaction(&tx_input, &outputs, None, Some(true))?;
                client
                    .sign_raw_transaction_with_wallet(&tx, None, None)?
                    .transaction()?
            };

            Ok(Self {
                env,
                graph,
                tx_send,
                tx_cancel,
            })
        }

        /// Rudimentary sync implementation.
        ///
        /// Scans through all transactions in the blockchain + mempool.
        fn sync(&mut self) -> anyhow::Result<()> {
            let client = self.env.rpc_client();
            for height in 0..=client.get_block_count()? {
                let hash = client.get_block_hash(height)?;
                let block = client.get_block(&hash)?;
                let _ = self.graph.apply_block_relevant(&block, height as _);
            }
            let _ = self.graph.batch_insert_relevant_unconfirmed(
                client
                    .get_raw_mempool()?
                    .into_iter()
                    .map(|txid| client.get_raw_transaction(&txid, None).map(|tx| (tx, 0)))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            Ok(())
        }

        /// Broadcast the original sending transaction.
        fn broadcast_send(&self) -> anyhow::Result<Txid> {
            let client = self.env.rpc_client();
            Ok(client.send_raw_transaction(&self.tx_send)?)
        }

        /// Broadcast the cancellation transaction.
        fn broadcast_cancel(&self) -> anyhow::Result<Txid> {
            let client = self.env.rpc_client();
            Ok(client.send_raw_transaction(&self.tx_cancel)?)
        }
    }

    // Broadcast `tx_send`.
    // Sync.
    // Broadcast `tx_cancel`.
    // `tx_cancel` gets confirmed.
    // Sync.
    // Expect: Both `tx_send` and `tx_cancel` appears in `recv_graph`.
    {
        let mut env = ScenarioEnv::new()?;
        let send_txid = env.broadcast_send()?;
        env.sync()?;
        let cancel_txid = env.broadcast_cancel()?;
        env.env.mine_blocks(6, None)?;
        env.sync()?;

        assert_eq!(env.graph.graph().full_txs().count(), 2);
        assert!(env.graph.graph().get_tx(send_txid).is_some());
        assert!(env.graph.graph().get_tx(cancel_txid).is_some());
    }

    // Broadcast `tx_send`.
    // Sync.
    // Broadcast `tx_cancel`.
    // Sync.
    // Expect: Both `tx_send` and `tx_cancel` appears in `recv_graph`.
    {
        let mut env = ScenarioEnv::new()?;
        let send_txid = env.broadcast_send()?;
        env.sync()?;
        let cancel_txid = env.broadcast_cancel()?;
        env.sync()?;

        assert_eq!(env.graph.graph().full_txs().count(), 2);
        assert!(env.graph.graph().get_tx(send_txid).is_some());
        assert!(env.graph.graph().get_tx(cancel_txid).is_some());
    }

    // If we don't see `tx_send` in the first place, `tx_cancel` should not be relevant.
    {
        let mut env = ScenarioEnv::new()?;
        let _ = env.broadcast_send()?;
        let _ = env.broadcast_cancel()?;
        env.sync()?;

        assert_eq!(env.graph.graph().full_txs().count(), 0);
    }

    Ok(())
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
    use bdk_chain::indexer::keychain_txout;
    let (descriptor, _) = Descriptor::parse_descriptor(&Secp256k1::signing_only(), DESCRIPTORS[0])
        .expect("must be valid");
    let spk_0 = descriptor.at_derivation_index(0).unwrap().script_pubkey();
    let spk_1 = descriptor.at_derivation_index(9).unwrap().script_pubkey();
    let lookahead = 10;

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<()>>::new({
        let mut indexer = KeychainTxOutIndex::new(lookahead, true);
        let is_inserted = indexer.insert_descriptor((), descriptor.clone()).unwrap();
        assert!(is_inserted);
        indexer
    });

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
            spk_cache: [(descriptor.descriptor_id(), {
                let index_after_spk_1 = 9 /* index of spk_1 */ + 1;
                SpkIterator::new_with_range(
                    &descriptor,
                    // This will also persist the staged spk cache inclusions from prev call to
                    // `.insert_descriptor`.
                    0..index_after_spk_1 + lookahead,
                )
                .collect()
            })]
            .into(),
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
            spk_cache: [(
                descriptor.descriptor_id(),
                SpkIterator::new_with_range(&descriptor, 0..=9 /* index of spk_1*/  + lookahead)
                    .collect(),
            )]
            .into(),
        },
    };

    assert_eq!(graph.initial_changeset(), initial_changeset);
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

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<String>>::new({
        let mut indexer = KeychainTxOutIndex::new(10, true);
        assert!(indexer
            .insert_descriptor("keychain_1".into(), desc_1)
            .unwrap());
        assert!(indexer
            .insert_descriptor("keychain_2".into(), desc_2)
            .unwrap());
        indexer
    });

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
                .unwrap_or_else(|| panic!("block must exist at {height}"));
            let txouts = graph
                .graph()
                .filter_chain_txouts(
                    &local_chain,
                    chain_tip,
                    CanonicalizationParams::default(),
                    graph.index.outpoints().iter().cloned(),
                )
                .collect::<Vec<_>>();

            let utxos = graph
                .graph()
                .filter_chain_unspents(
                    &local_chain,
                    chain_tip,
                    CanonicalizationParams::default(),
                    graph.index.outpoints().iter().cloned(),
                )
                .collect::<Vec<_>>();

            let balance = graph.graph().balance(
                &local_chain,
                chain_tip,
                CanonicalizationParams::default(),
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
            .list_canonical_txs(
                chain,
                chain.tip().block_id(),
                CanonicalizationParams::default(),
            )
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
            exp_pos: Some(ChainPosition::Unconfirmed {
                last_seen: Some(2),
                first_seen: Some(2),
            }),
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
            exp_pos: Some(ChainPosition::Unconfirmed {
                last_seen: Some(2),
                first_seen: Some(2),
            }),
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
            exp_pos: Some(ChainPosition::Unconfirmed {
                last_seen: None,
                first_seen: None,
            }),
        },
    ]
    .into_iter()
    .for_each(|t| run(&chain, &mut graph, t));
}
