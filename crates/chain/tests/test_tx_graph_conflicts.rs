#![cfg(feature = "miniscript")]

#[macro_use]
mod common;

use bdk_chain::{Balance, BlockId};
use bdk_testenv::{block_id, hash, local_chain};
use bitcoin::{Amount, OutPoint, ScriptBuf};
use common::*;
use std::collections::{BTreeSet, HashSet};

#[allow(dead_code)]
struct Scenario<'a> {
    /// Name of the test scenario
    name: &'a str,
    /// Transaction templates
    tx_templates: &'a [TxTemplate<'a, BlockId>],
    /// Names of txs that must exist in the output of `list_canonical_txs`
    exp_chain_txs: HashSet<&'a str>,
    /// Outpoints that must exist in the output of `filter_chain_txouts`
    exp_chain_txouts: HashSet<(&'a str, u32)>,
    /// Outpoints of UTXOs that must exist in the output of `filter_chain_unspents`
    exp_unspents: HashSet<(&'a str, u32)>,
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
    let local_chain = local_chain!(
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "unconfirmed_coinbase",
                    inputs: &[TxInTemplate::Coinbase],
                    outputs: &[TxOutTemplate::new(5000, Some(0))],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "confirmed_genesis",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(1))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "unconfirmed_conflict",
                    inputs: &[
                        TxInTemplate::PrevTx("confirmed_genesis", 0), 
                        TxInTemplate::PrevTx("unconfirmed_coinbase", 0)
                    ],
                    outputs: &[TxOutTemplate::new(20000, Some(2))],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "confirmed_conflict",
                    inputs: &[TxInTemplate::PrevTx("confirmed_genesis", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(3))],
                    anchors: &[block_id!(4, "E")],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "tx1",
                    outputs: &[TxOutTemplate::new(40000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_1",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(2))],
                    last_seen: Some(300),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_2",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(3))],
                    last_seen: Some(300),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "tx1",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0)), TxOutTemplate::new(10000, Some(1))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_1",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(2))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_2",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0),  TxInTemplate::PrevTx("tx1", 1)],
                    outputs: &[TxOutTemplate::new(30000, Some(3))],
                    last_seen: Some(300),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "tx1",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_1",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_2",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(2))],
                    last_seen: Some(300),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_3",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(40000, Some(3))],
                    last_seen: Some(400),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "tx1",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_1",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_orphaned_conflict",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(2))],
                    anchors: &[block_id!(4, "Orphaned Block")],
                    last_seen: Some(300),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "tx1",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_1",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_orphaned_conflict",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(2))],
                    anchors: &[block_id!(4, "Orphaned Block")],
                    last_seen: Some(100),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "tx1",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_1",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_2",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(2))],
                    last_seen: Some(300),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_conflict_3",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(40000, Some(3))],
                    last_seen: Some(400),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx_confirmed_conflict",
                    inputs: &[TxInTemplate::PrevTx("tx1", 0)],
                    outputs: &[TxOutTemplate::new(50000, Some(4))],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    last_seen: Some(22),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(23),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B'",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(2))],
                    last_seen: Some(24),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "C",
                    inputs: &[TxInTemplate::PrevTx("B", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(3))],
                    last_seen: Some(25),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B'",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(2))],
                    anchors: &[block_id!(4, "E")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "C",
                    inputs: &[TxInTemplate::PrevTx("B", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(3))],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(2),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B'",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(2))],
                    anchors: &[block_id!(4, "E")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "C",
                    inputs: &[TxInTemplate::PrevTx("B'", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(3))],
                    last_seen: Some(1),
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("A", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B'",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(30000, Some(2))],
                    last_seen: Some(300),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "C",
                    inputs: &[
                        TxInTemplate::PrevTx("B", 0),
                        TxInTemplate::PrevTx("B'", 0),
                    ],
                    outputs: &[TxOutTemplate::new(20000, Some(3))],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("A", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B'",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(50000, Some(4))],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "C",
                    inputs: &[
                        TxInTemplate::PrevTx("B", 0),
                        TxInTemplate::PrevTx("B'", 0),
                    ],
                    outputs: &[TxOutTemplate::new(20000, Some(5))],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    last_seen: None,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("A", 0), TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(20000, Some(1))],
                    last_seen: Some(200),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B'",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(50000, Some(4))],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "C",
                    inputs: &[
                        TxInTemplate::PrevTx("B", 0),
                        TxInTemplate::PrevTx("B'", 0),
                    ],
                    outputs: &[TxOutTemplate::new(20000, Some(5))],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "D",
                    inputs: &[TxInTemplate::PrevTx("C", 0)],
                    outputs: &[TxOutTemplate::new(20000, Some(6))],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "first",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(1000, Some(0))],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "second",
                    inputs: &[TxInTemplate::PrevTx("first", 0)],
                    outputs: &[TxOutTemplate::new(900, Some(0))],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "anchored",
                    inputs: &[TxInTemplate::PrevTx("second", 0)],
                    outputs: &[TxOutTemplate::new(800, Some(0))],
                    anchors: &[block_id!(3, "D")],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "root",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10_000, Some(0))],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "last_seen_conflict",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(9900, Some(1))],
                    last_seen: Some(1000),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "transitively_anchored_conflict",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(9000, Some(1))],
                    last_seen: Some(100),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "anchored",
                    inputs: &[TxInTemplate::PrevTx("transitively_anchored_conflict", 0)],
                    outputs: &[TxOutTemplate::new(8000, Some(2))],
                    anchors: &[block_id!(4, "E")],
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "root",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10_000, None)],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "tx",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(9000, Some(0))],
                    anchors: &[block_id!(6, "not G")],
                    ..Default::default()
                },
            ],
            exp_chain_txs: HashSet::from(["root", "tx"]),
            exp_chain_txouts: HashSet::from([("tx", 0)]),
            exp_unspents: HashSet::from([("tx", 0)]),
            exp_balance: Balance { trusted_pending: Amount::from_sat(9000), ..Default::default() }
        },
        Scenario {
            name: "tx spends from 2 conflicting transactions where a conflict spends another",
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10_000, None)],
                    last_seen: Some(1),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "S1",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(9_000, None)],
                    last_seen: Some(2),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "S2",
                    inputs: &[TxInTemplate::PrevTx("A", 0), TxInTemplate::PrevTx("S1", 0)],
                    outputs: &[TxOutTemplate::new(17_000, None)],
                    last_seen: Some(3),
                    ..Default::default()
                },
            ],
            exp_chain_txs: HashSet::from(["A", "S1"]),
            exp_chain_txouts: HashSet::from([]),
            exp_unspents: HashSet::from([]),
            exp_balance: Balance::default(),
        },
        Scenario {
            name: "tx spends from 2 conflicting transactions where the conflict is nested",
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10_000, Some(0))],
                    last_seen: Some(1),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "S1",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(9_000, Some(0))],
                    last_seen: Some(3),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("S1", 0)],
                    outputs: &[TxOutTemplate::new(8_000, Some(0))],
                    last_seen: Some(2),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "S2",
                    inputs: &[TxInTemplate::PrevTx("B", 0), TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(17_000, Some(0))],
                    last_seen: Some(4),
                    ..Default::default()
                },
            ],
            exp_chain_txs: HashSet::from(["A", "S1", "B"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B", 0), ("S1", 0)]),
            exp_unspents: HashSet::from([("B", 0)]),
            exp_balance: Balance { trusted_pending: Amount::from_sat(8_000), ..Default::default() },
        },
        Scenario {
            name: "tx spends from 2 conflicting transactions where the conflict is nested (different last_seens)",
            tx_templates: &[
                TxTemplate {
                    tx_name: "A",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[TxOutTemplate::new(10_000, Some(0))],
                    last_seen: Some(1),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "S1",
                    inputs: &[TxInTemplate::PrevTx("A", 0)],
                    outputs: &[TxOutTemplate::new(9_000, Some(0))],
                    last_seen: Some(4),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "B",
                    inputs: &[TxInTemplate::PrevTx("S1", 0)],
                    outputs: &[TxOutTemplate::new(8_000, Some(0))],
                    last_seen: Some(2),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "S2",
                    inputs: &[TxInTemplate::PrevTx("A", 0), TxInTemplate::PrevTx("B", 0)],
                    outputs: &[TxOutTemplate::new(17_000, Some(0))],
                    last_seen: Some(3),
                    ..Default::default()
                },
            ],
            exp_chain_txs: HashSet::from(["A", "S1", "B"]),
            exp_chain_txouts: HashSet::from([("A", 0), ("B", 0), ("S1", 0)]),
            exp_unspents: HashSet::from([("B", 0)]),
            exp_balance: Balance { trusted_pending: Amount::from_sat(8_000), ..Default::default() },
        },
        Scenario {
            name: "assume-canonical-tx displaces unconfirmed chain",
            tx_templates: &[
                TxTemplate {
                    tx_name: "root",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[
                        TxOutTemplate::new(21_000, Some(0)),
                        TxOutTemplate::new(21_000, Some(1)),
                    ],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "unconfirmed",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(20_000, Some(1))],
                    last_seen: Some(2),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "unconfirmed_descendant",
                    inputs: &[
                        TxInTemplate::PrevTx("unconfirmed", 0),
                        TxInTemplate::PrevTx("root", 1),
                    ],
                    outputs: &[TxOutTemplate::new(28_000, Some(2))],
                    last_seen: Some(2),
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "assume_canonical",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(19_000, Some(3))],
                    assume_canonical: true,
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "root",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[
                        TxOutTemplate::new(21_000, Some(0)),
                        TxOutTemplate::new(21_000, Some(1)),
                    ],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "confirmed",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(20_000, Some(1))],
                    anchors: &[block_id!(2, "C")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "confirmed_descendant",
                    inputs: &[
                        TxInTemplate::PrevTx("confirmed", 0),
                        TxInTemplate::PrevTx("root", 1),
                    ],
                    outputs: &[TxOutTemplate::new(28_000, Some(2))],
                    anchors: &[block_id!(3, "D")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "assume_canonical",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(19_000, Some(3))],
                    assume_canonical: true,
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "root",
                    inputs: &[TxInTemplate::Bogus],
                    outputs: &[
                        TxOutTemplate::new(21_000, Some(0)),
                    ],
                    anchors: &[block_id!(1, "B")],
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "assume_a",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(20_000, Some(1))],
                    assume_canonical: true,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "assume_b",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(19_000, Some(1))],
                    assume_canonical: true,
                    ..Default::default()
                },
                TxTemplate {
                    tx_name: "assume_c",
                    inputs: &[TxInTemplate::PrevTx("root", 0)],
                    outputs: &[TxOutTemplate::new(18_000, Some(1))],
                    assume_canonical: true,
                    ..Default::default()
                },
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
            tx_templates: &[
                TxTemplate {
                    tx_name: "coinbase",
                    inputs: &[TxInTemplate::Coinbase],
                    outputs: &[TxOutTemplate::new(21_000, Some(0))],
                    // Stale block
                    anchors: &[block_id!(1, "B-prime")],
                    ..Default::default()
                }
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
        let env = init_graph(scenario.tx_templates.iter());

        let txs = env
            .tx_graph
            .list_canonical_txs(&local_chain, chain_tip, env.canonicalization_params.clone())
            .map(|tx| tx.tx_node.txid)
            .collect::<BTreeSet<_>>();
        let exp_txs = scenario
            .exp_chain_txs
            .iter()
            .map(|txid| *env.txid_to_name.get(txid).expect("txid must exist"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            txs, exp_txs,
            "\n[{}] 'list_canonical_txs' failed",
            scenario.name
        );

        let txouts = env
            .tx_graph
            .filter_chain_txouts(
                &local_chain,
                chain_tip,
                env.canonicalization_params.clone(),
                env.indexer.outpoints().iter().cloned(),
            )
            .map(|(_, full_txout)| full_txout.outpoint)
            .collect::<BTreeSet<_>>();
        let exp_txouts = scenario
            .exp_chain_txouts
            .iter()
            .map(|(txid, vout)| OutPoint {
                txid: *env.txid_to_name.get(txid).expect("txid must exist"),
                vout: *vout,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            txouts, exp_txouts,
            "\n[{}] 'filter_chain_txouts' failed",
            scenario.name
        );

        let utxos = env
            .tx_graph
            .filter_chain_unspents(
                &local_chain,
                chain_tip,
                env.canonicalization_params.clone(),
                env.indexer.outpoints().iter().cloned(),
            )
            .map(|(_, full_txout)| full_txout.outpoint)
            .collect::<BTreeSet<_>>();
        let exp_utxos = scenario
            .exp_unspents
            .iter()
            .map(|(txid, vout)| OutPoint {
                txid: *env.txid_to_name.get(txid).expect("txid must exist"),
                vout: *vout,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            utxos, exp_utxos,
            "\n[{}] 'filter_chain_unspents' failed",
            scenario.name
        );

        let balance = env.tx_graph.balance(
            &local_chain,
            chain_tip,
            env.canonicalization_params.clone(),
            env.indexer.outpoints().iter().cloned(),
            |_, spk: ScriptBuf| env.indexer.index_of_spk(spk).is_some(),
        );
        assert_eq!(
            balance, scenario.exp_balance,
            "\n[{}] 'balance' failed",
            scenario.name
        );
    }
}
