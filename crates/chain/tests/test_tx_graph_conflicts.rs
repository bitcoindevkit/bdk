#![cfg(feature = "miniscript")]

use bdk_chain::{local_chain::LocalChain, Balance, BlockId};
use bdk_testenv::{block_id, hash, local_chain};
use bdk_testenv::{init_graph, TxInTemplate, TxOutTemplate, TxTemplate};
use bitcoin::{Amount, BlockHash, OutPoint};
use std::collections::{BTreeSet, HashSet};

#[allow(dead_code)]
struct Scenario {
    /// Name of the test scenario
    name: &'static str,
    /// Transaction templates
    tx_templates: Vec<TxTemplate<BlockId>>,
    /// Names of txs that must exist in the output of `list_canonical_txs`
    exp_chain_txs: HashSet<&'static str>,
    /// Outpoints that must exist in the output of `filter_chain_txouts`
    exp_chain_txouts: HashSet<(&'static str, u32)>,
    /// Outpoints of UTXOs that must exist in the output of `filter_chain_unspents`
    exp_unspents: HashSet<(&'static str, u32)>,
    /// Expected balances
    exp_balance: Balance,
}

/// This test ensures that [`TxGraph`] will reliably filter out irrelevant transactions when
/// presented with multiple conflicting transaction scenarios using the [`TxTemplate`] structure.
/// This test also checks that [`TxGraph::list_canonical_txs`], [`TxGraph::filter_chain_txouts`],
/// [`TxGraph::filter_chain_unspents`], and [`TxGraph::balance`] return correct data.
#[test]
fn test_tx_conflict_handling() {
    // Create Local chains
    let local_chain: LocalChain<BlockHash> = local_chain!(
        (0, hash!("A")),
        (1, hash!("B")),
        (2, hash!("C")),
        (3, hash!("D")),
        (4, hash!("E")),
        (5, hash!("F")),
        (6, hash!("G"))
    );
    let chain_tip = local_chain.tip().block_id();

    let scenarios = [
        Scenario {
            name: "coinbase tx cannot be in mempool and be unconfirmed",
            tx_templates: vec![
                TxTemplate::new("unconfirmed_coinbase")
                    .with_inputs(vec![TxInTemplate::Coinbase])
                    .with_outputs(vec![TxOutTemplate::new(5_000, Some(0))]),
                TxTemplate::new("confirmed_genesis")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(1))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("unconfirmed_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("confirmed_genesis".into(), 0),
                                TxInTemplate::PrevTx("unconfirmed_coinbase".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(2))]),
                TxTemplate::new("confirmed_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("confirmed_genesis".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(3))])
                    .with_anchors(vec![block_id!(4, "E")]),
            ],
            exp_chain_txs: HashSet::from(["confirmed_genesis", "confirmed_conflict"]),
            exp_chain_txouts: HashSet::from([("confirmed_genesis", 0), ("confirmed_conflict", 0)]),
            exp_unspents: HashSet::from([("confirmed_conflict", 0)]),
            exp_balance: Balance {
                confirmed: Amount::from_sat(20000),
                ..Default::default()
            },
        },
        Scenario {
            name: "2 unconfirmed txs with same last_seens conflict",
            tx_templates: vec![
                TxTemplate::new("tx1")
                    .with_outputs(vec![TxOutTemplate::new(40_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx_conflict_1")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(2))])
                    .with_last_seen(Some(300)),
                TxTemplate::new("tx_conflict_2")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30000, Some(3))])
                    .with_last_seen(Some(300)),
            ],
            // the txgraph is going to pick tx_conflict_2 because of higher lexicographical txid
            exp_chain_txs: HashSet::from(["tx1", "tx_conflict_2"]),
            exp_chain_txouts: HashSet::from([("tx1", 0), ("tx_conflict_2", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_2", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(30000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "2 unconfirmed txs with different last_seens conflict",
            tx_templates: vec![
                TxTemplate::new("tx1")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0)), TxOutTemplate::new(10_000, Some(1))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx_conflict_1")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(2))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("tx_conflict_2")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0), TxInTemplate::PrevTx("tx1".into(), 1)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(3))])
                    .with_last_seen(Some(300))
            ],
            exp_chain_txs: HashSet::from(["tx1", "tx_conflict_2"]),
            exp_chain_txouts: HashSet::from([("tx1", 0), ("tx1", 1), ("tx_conflict_2", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_2", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(30000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "3 unconfirmed txs with different last_seens conflict",
            tx_templates: vec![
                TxTemplate::new("tx1")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx_conflict_1")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("tx_conflict_2")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30000, Some(2))])
                    .with_last_seen(Some(300)),
                TxTemplate::new("tx_conflict_3")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(40000, Some(3))])
                    .with_last_seen(Some(400)),
            ],
            exp_chain_txs: HashSet::from(["tx1", "tx_conflict_3"]),
            exp_chain_txouts: HashSet::from([("tx1", 0), ("tx_conflict_3", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_3", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(40000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "unconfirmed tx conflicts with tx in orphaned block, orphaned higher last_seen",
            tx_templates: vec![
                TxTemplate::new("tx1")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx_conflict_1")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("tx_orphaned_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(2))])
                    .with_anchors(vec![block_id!(4, "Orphaned Block")])
                    .with_last_seen(Some(300)),
            ],
            exp_chain_txs: HashSet::from(["tx1", "tx_orphaned_conflict"]),
            exp_chain_txouts: HashSet::from([("tx1", 0), ("tx_orphaned_conflict", 0)]),
            exp_unspents: HashSet::from([("tx_orphaned_conflict", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(30000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "unconfirmed tx conflicts with tx in orphaned block, orphaned lower last_seen",
            tx_templates: vec![
                TxTemplate::new("tx1")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx_conflict_1")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("tx_orphaned_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(2))])
                    .with_anchors(vec![block_id!(4, "Orphaned Block")])
                    .with_last_seen(Some(100)),
            ],
            exp_chain_txs: HashSet::from(["tx1", "tx_conflict_1"]),
            exp_chain_txouts: HashSet::from([("tx1", 0), ("tx_conflict_1", 0)]),
            exp_unspents: HashSet::from([("tx_conflict_1", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(20000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "multiple unconfirmed txs conflict with a confirmed tx",
            tx_templates: vec![
                TxTemplate::new("tx1")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx_conflict_1")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("tx_conflict_2")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(2))])
                    .with_last_seen(Some(300)),
                TxTemplate::new("tx_conflict_3")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(40_000, Some(3))])
                    .with_last_seen(Some(400)),
                TxTemplate::new("tx_confirmed_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("tx1".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(50_000, Some(4))])
                    .with_anchors(vec![block_id!(1, "B")]),
            ],
            exp_chain_txs: HashSet::from(["tx1", "tx_confirmed_conflict"]),
            exp_chain_txouts: HashSet::from([("tx1", 0), ("tx_confirmed_conflict", 0)]),
            exp_unspents: HashSet::from([("tx_confirmed_conflict", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::ZERO,
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(50000),
            },
        },
        Scenario {
            name: "B and B' spend A and conflict, C spends B, all the transactions are unconfirmed, B' has higher last_seen than B",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_last_seen(Some(22)),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(23)),
                TxTemplate::new("B'")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(2))])
                    .with_last_seen(Some(24)),
                TxTemplate::new("C")
                    .with_inputs(vec![TxInTemplate::PrevTx("B".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(3))])
                    .with_last_seen(Some(25))
            ],
            // A, B, C will appear in the list methods
            // This is because B' has a higher last seen than B, but C has a higher
            // last seen than B', so B and C are considered canonical
            exp_chain_txs: HashSet::from(["A", "B", "C"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B", 0), ("C", 0)]),
            exp_unspents: HashSet::from([("C", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(30000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "B and B' spend A and conflict, C spends B, A and B' are in best chain",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(&[TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))]),
                TxTemplate::new("B'")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20000, Some(2))])
                    .with_anchors(vec![block_id!(4, "E")]),
                TxTemplate::new("C")
                    .with_inputs(vec![TxInTemplate::PrevTx("B".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(3))]),
            ],
            // B and C should not appear in the list methods
            exp_chain_txs: HashSet::from(["A", "B'"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B'", 0)]),
            exp_unspents: HashSet::from([("B'", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::ZERO,
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(20000),
            },
        },
        Scenario {
            name: "B and B' spend A and conflict, C spends B', A and B' are in best chain",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(2)),
                TxTemplate::new("B'")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(2))])
                    .with_anchors(vec![block_id!(4, "E")]),
                TxTemplate::new("C")
                    .with_inputs(vec![TxInTemplate::PrevTx("B'".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(3))])
                    .with_last_seen(Some(1))
            ],
            // B should not appear in the list methods
            exp_chain_txs: HashSet::from(["A", "B'", "C"]),
            exp_chain_txouts: HashSet::from([
                ("A", 0),
                ("B'", 0),
                ("C", 0),
            ]),
            exp_unspents: HashSet::from([("C", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(30000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "B and B' spend A and conflict, C spends both B and B', A is in best chain",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(vec![TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("B'")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(30_000, Some(2))])
                    .with_last_seen(Some(300)),
                TxTemplate::new("C")
                    .with_inputs(vec![TxInTemplate::PrevTx("B".into(), 0), TxInTemplate::PrevTx("B'".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(3))]),
            ],
            // C should not appear in the list methods
            exp_chain_txs: HashSet::from(["A", "B'"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B'", 0)]),
            exp_unspents: HashSet::from([("B'", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(30000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "B and B' spend A and conflict, B' is confirmed, C spends both B and B', A is in best chain",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(&[TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("B'")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(50_000, Some(4))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("C")
                    .with_inputs(vec![TxInTemplate::PrevTx("B".into(), 0), TxInTemplate::PrevTx("B'".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(5))]),
            ],
            // C should not appear in the list methods
            exp_chain_txs: HashSet::from(["A", "B'"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B'", 0)]),
            exp_unspents: HashSet::from([("B'", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::ZERO,
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(50000),
            },
        },
        Scenario {
            name: "B and B' spend A and conflict, B' is confirmed, C spends both B and B', D spends C, A is in best chain",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(&[TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0), TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(200)),
                TxTemplate::new("B'")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(50_000, Some(4))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("C")
                    .with_inputs(vec![TxInTemplate::PrevTx("B".into(), 0),  TxInTemplate::PrevTx("B'".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(5))]),
                TxTemplate::new("D")
                    .with_inputs(vec![TxInTemplate::PrevTx("C".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(6))]),
            ],
            // D should not appear in the list methods
            exp_chain_txs: HashSet::from(["A", "B'"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B'", 0)]),
            exp_unspents: HashSet::from([("B'", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::ZERO,
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(50000),
            },
        },
        Scenario {
            name: "transitively confirmed ancestors",
            tx_templates: vec![
                TxTemplate::new("first")
                    .with_inputs(&[TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(1_000, Some(0))]),
                TxTemplate::new("second")
                    .with_inputs(vec![TxInTemplate::PrevTx("first".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(900, Some(0))]),
                TxTemplate::new("anchored")
                    .with_inputs(vec![TxInTemplate::PrevTx("second".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(800, Some(0))])
                    .with_anchors(vec![block_id!(3, "D")]),
            ],
            exp_chain_txs: HashSet::from(["first", "second", "anchored"]),
            exp_chain_txouts: HashSet::from([("first", 0), ("second", 0), ("anchored", 0)]),
            exp_unspents: HashSet::from([("anchored", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::ZERO,
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(800),
            }
        },
        Scenario {
            name: "transitively anchored txs should have priority over last seen",
            tx_templates: vec![
                TxTemplate::new("root")
                    .with_inputs(&[TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("last_seen_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(9_900, Some(1))])
                    .with_last_seen(Some(1_000)),
                TxTemplate::new("transitively_anchored_conflict")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(9_000, Some(1))])
                    .with_last_seen(Some(100)),
                TxTemplate::new("anchored")
                    .with_inputs(vec![TxInTemplate::PrevTx("transitively_anchored_conflict".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(8_000, Some(2))])
                    .with_anchors(vec![block_id!(4, "E")]),
            ],
            exp_chain_txs: HashSet::from(["root", "transitively_anchored_conflict", "anchored"]),
            exp_chain_txouts: HashSet::from([("root", 0), ("transitively_anchored_conflict", 0), ("anchored", 0)]),
            exp_unspents: HashSet::from([("anchored", 0)]),
            exp_balance: Balance {
                confirmed: Amount::from_sat(8000),
                ..Default::default()
            }
        },
        Scenario {
            name: "tx anchored in orphaned block and not seen in mempool should be canon",
            tx_templates: vec![
                TxTemplate::new("root")
                    .with_inputs(vec![TxInTemplate::Bogus ])
                    .with_outputs(vec![TxOutTemplate::new(10_000, None)])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("tx")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(9_000, Some(0))])
                    .with_anchors(vec![block_id!(6, "not G")]),
            ],
            exp_chain_txs: HashSet::from(["root", "tx"]),
            exp_chain_txouts: HashSet::from([("tx", 0)]),
            exp_unspents: HashSet::from([("tx", 0)]),
            exp_balance: Balance { trusted_pending: Amount::from_sat(9000), ..Default::default() }
        },
        Scenario {
            name: "tx spends from 2 conflicting transactions where a conflict spends another",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(&[TxInTemplate::Bogus ])
                    .with_outputs(vec![TxOutTemplate::new(10_000, None)])
                    .with_last_seen(Some(1)),
                TxTemplate::new("S1")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(9_000, None)])
                    .with_last_seen(Some(2)),
                TxTemplate::new("S2")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0), TxInTemplate::PrevTx("S1".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(17_000, None)])
                    .with_last_seen(Some(3)),
            ],
            exp_chain_txs: HashSet::from(["A", "S1"]),
            exp_chain_txouts: HashSet::from([]),
            exp_unspents: HashSet::from([]),
            exp_balance: Balance::default(),
        },
        Scenario {
            name: "tx spends from 2 conflicting transactions where the conflict is nested",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(&[TxInTemplate::Bogus ])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_last_seen(Some(1)),
                TxTemplate::new("S1")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(9_000, Some(0))])
                    .with_last_seen(Some(3)),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("S1".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(8_000, Some(0))])
                    .with_last_seen(Some(2)),
                TxTemplate::new("S2")
                    .with_inputs(vec![TxInTemplate::PrevTx("B".into(), 0), TxInTemplate::PrevTx("A".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(17_000, Some(0))])
                    .with_last_seen(Some(4)),
            ],
            exp_chain_txs: HashSet::from(["A", "S1", "B"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B", 0), ("S1", 0)]),
            exp_unspents: HashSet::from([("B", 0)]),
            exp_balance: Balance { trusted_pending: Amount::from_sat(8_000), ..Default::default() },
        },
        Scenario {
            name: "tx spends from 2 conflicting transactions where the conflict is nested (different last_seens)",
            tx_templates: vec![
                TxTemplate::new("A")
                    .with_inputs(&[TxInTemplate::Bogus ])
                    .with_outputs(vec![TxOutTemplate::new(10_000, Some(0))])
                    .with_last_seen(Some(1)),
                TxTemplate::new("S1")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(9_000, Some(0))])
                    .with_last_seen(Some(4)),
                TxTemplate::new("B")
                    .with_inputs(vec![TxInTemplate::PrevTx("S1".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(8_000, Some(0))])
                    .with_last_seen(Some(2)),
                TxTemplate::new("S2")
                    .with_inputs(vec![TxInTemplate::PrevTx("A".into(), 0), TxInTemplate::PrevTx("B".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(17_000, Some(0))])
                    .with_last_seen(Some(3)),
            ],
            exp_chain_txs: HashSet::from(["A", "S1", "B"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B", 0), ("S1", 0)]),
            exp_unspents: HashSet::from([("B", 0)]),
            exp_balance: Balance { trusted_pending: Amount::from_sat(8_000), ..Default::default() },
        },
        Scenario {
            name: "assume-canonical-tx displaces unconfirmed chain",
            tx_templates: vec![
                TxTemplate::new("root")
                    .with_inputs(&[TxInTemplate::Bogus ])
                    .with_outputs(vec![TxOutTemplate::new(21_000, Some(0)),
                TxOutTemplate::new(21_000, Some(1))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("unconfirmed")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0) ])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_last_seen(Some(2)),
                TxTemplate::new("unconfirmed_descendant")
                    .with_inputs(vec![TxInTemplate::PrevTx("unconfirmed".into(), 0),
                TxInTemplate::PrevTx("root".into(), 1) ])
                    .with_outputs(vec![TxOutTemplate::new(28_000, Some(2))])
                    .with_last_seen(Some(2)),
                TxTemplate::new("assume_canonical")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(19_000, Some(3))])
                    .with_assume_canonical(true),
            ],
            exp_chain_txs: HashSet::from(["root", "assume_canonical"]),
            exp_chain_txouts: HashSet::from([("root", 0), ("root", 1), ("assume_canonical", 0)]),
            exp_unspents: HashSet::from([("root", 1), ("assume_canonical", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(19_000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(21_000),
            },
        },
        Scenario {
            name: "assume-canonical-tx displaces confirmed chain",
            tx_templates: vec![
                TxTemplate::new("root")
                    .with_inputs(&[TxInTemplate::Bogus ])
                    .with_outputs(vec![TxOutTemplate::new(21_000, Some(0)),
                TxOutTemplate::new(21_000, Some(1))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("confirmed")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_anchors(vec![block_id!(2, "C")]),
                TxTemplate::new("confirmed_descendant")
                    .with_inputs(vec![TxInTemplate::PrevTx("confirmed".into(), 0), TxInTemplate::PrevTx("root".into(), 1)])
                    .with_outputs(vec![TxOutTemplate::new(28_000, Some(2))])
                    .with_anchors(vec![block_id!(3, "D")]),
                TxTemplate::new("assume_canonical")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(19_000, Some(3))])
                    .with_assume_canonical(true),
            ],
            exp_chain_txs: HashSet::from(["root", "assume_canonical"]),
            exp_chain_txouts: HashSet::from([("root", 0), ("root", 1), ("assume_canonical", 0)]),
            exp_unspents: HashSet::from([("root", 1), ("assume_canonical", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(19_000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::from_sat(21_000),
            },
        },
        Scenario {
            name: "assume-canonical txs respects order",
            tx_templates: vec![
                TxTemplate::new("root")
                    .with_inputs(&[TxInTemplate::Bogus])
                    .with_outputs(vec![TxOutTemplate::new(21_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B")]),
                TxTemplate::new("assume_a")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(20_000, Some(1))])
                    .with_assume_canonical(true),
                TxTemplate::new("assume_b")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(19_000, Some(1))])
                    .with_assume_canonical(true),
                TxTemplate::new("assume_c")
                    .with_inputs(vec![TxInTemplate::PrevTx("root".into(), 0)])
                    .with_outputs(vec![TxOutTemplate::new(18_000, Some(1))])
                    .with_assume_canonical(true),
            ],
            exp_chain_txs: HashSet::from(["root", "assume_c"]),
            exp_chain_txouts: HashSet::from([("root", 0), ("assume_c", 0)]),
            exp_unspents: HashSet::from([("assume_c", 0)]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::from_sat(18_000),
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            },
        },
        Scenario {
            name: "coinbase tx must not become unconfirmed",
            tx_templates: vec![
                TxTemplate::new("coinbase")
                    .with_inputs(&[TxInTemplate::Coinbase])
                    .with_outputs(vec![TxOutTemplate::new(21_000, Some(0))])
                    .with_anchors(vec![block_id!(1, "B-prime")])
            ],
            exp_chain_txs: HashSet::from([]),
            exp_chain_txouts: HashSet::from([]),
            exp_unspents: HashSet::from([]),
            exp_balance: Balance {
                immature: Amount::ZERO,
                trusted_pending: Amount::ZERO,
                untrusted_pending: Amount::ZERO,
                confirmed: Amount::ZERO,
            }
        }
    ];

    for scenario in scenarios {
        let env = init_graph(scenario.tx_templates);

        let txs = env
            .tx_graph
            .canonical_view(&local_chain, chain_tip, env.canonicalization_params.clone())
            .txs()
            .map(|tx| tx.txid)
            .collect::<BTreeSet<_>>();
        let exp_txs = scenario
            .exp_chain_txs
            .iter()
            .map(|&txid| *env.txid_to_name.get(txid).expect("txid must exist"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            txs, exp_txs,
            "\n[{}] 'list_canonical_txs' failed",
            scenario.name
        );

        let txouts = env
            .tx_graph
            .canonical_view(&local_chain, chain_tip, env.canonicalization_params.clone())
            .filter_outpoints(env.indexer.outpoints().iter().cloned())
            .map(|(_, full_txout)| full_txout.outpoint)
            .collect::<BTreeSet<_>>();
        let exp_txouts = scenario
            .exp_chain_txouts
            .iter()
            .map(|&(txid, vout)| OutPoint {
                txid: *env.txid_to_name.get(txid).expect("txid must exist"),
                vout,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            txouts, exp_txouts,
            "\n[{}] 'filter_chain_txouts' failed",
            scenario.name
        );

        let utxos = env
            .tx_graph
            .canonical_view(&local_chain, chain_tip, env.canonicalization_params.clone())
            .filter_unspent_outpoints(env.indexer.outpoints().iter().cloned())
            .map(|(_, full_txout)| full_txout.outpoint)
            .collect::<BTreeSet<_>>();
        let exp_utxos = scenario
            .exp_unspents
            .iter()
            .map(|&(txid, vout)| OutPoint {
                txid: *env.txid_to_name.get(txid).expect("txid must exist"),
                vout,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            utxos, exp_utxos,
            "\n[{}] 'filter_chain_unspents' failed",
            scenario.name
        );

        let balance = env
            .tx_graph
            .canonical_view(&local_chain, chain_tip, env.canonicalization_params.clone())
            .balance(
                env.indexer.outpoints().iter().cloned(),
                |_, txout| {
                    env.indexer
                        .index_of_spk(txout.txout.script_pubkey.as_script())
                        .is_some()
                },
                0,
            );
        assert_eq!(
            balance, scenario.exp_balance,
            "\n[{}] 'balance' failed",
            scenario.name
        );
    }
}
