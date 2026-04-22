#![cfg(feature = "miniscript")]

mod common;

use bdk_chain::{BlockId, CanonicalReason, ChainPosition};
use bdk_testenv::{block_id, hash, local_chain};
use bitcoin::Txid;
use common::*;
use std::collections::HashSet;

struct Scenario<'a> {
    name: &'a str,
    tx_templates: &'a [TxTemplate<'a, BlockId>],
    exp_canonical_txs: HashSet<&'a str>,
}

#[test]
fn test_assumed_canonical_scenarios() {
    // Create a local chain
    let local_chain = local_chain![
        (0, hash!("genesis")),
        (1, hash!("block1")),
        (2, hash!("block2")),
        (3, hash!("block3")),
        (4, hash!("block4")),
        (5, hash!("block5")),
        (6, hash!("block6")),
        (7, hash!("block7")),
        (8, hash!("block8")),
        (9, hash!("block9")),
        (10, hash!("block10"))
    ];
    let chain_tip = local_chain.tip().block_id();

    // Create arrays before scenario to avoid lifetime issues
    let tx_templates = [
        TxTemplate {
            tx_name: "txA",
            inputs: &[TxInTemplate::Bogus],
            outputs: &[TxOutTemplate::new(100000, Some(0))],
            anchors: &[],
            last_seen: None,
            assume_canonical: false,
        },
        TxTemplate {
            tx_name: "txB",
            inputs: &[TxInTemplate::PrevTx("txA", 0)],
            outputs: &[TxOutTemplate::new(50000, Some(0))],
            anchors: &[block_id!(5, "block5")],
            last_seen: None,
            assume_canonical: false,
        },
        TxTemplate {
            tx_name: "txC",
            inputs: &[TxInTemplate::PrevTx("txB", 0)],
            outputs: &[TxOutTemplate::new(25000, Some(0))],
            anchors: &[],
            last_seen: None,
            assume_canonical: true,
        },
    ];

    let scenarios = vec![Scenario {
        name: "txC spends txB; txB spends txA; txB is anchored; txC is assumed canonical",
        tx_templates: &tx_templates,
        exp_canonical_txs: HashSet::from(["txA", "txB", "txC"]),
    }];

    for scenario in scenarios {
        let env = init_graph(scenario.tx_templates);

        // get the actual txid from given tx_name.
        let txid_c = *env.txid_to_name.get("txC").unwrap();

        // build the expected `CanonicalReason` with specific descendant txid's
        //
        // in this scenario: txC is assumed canonical, and it's descendant of txB and txA
        // therefore the whole chain should become assumed canonical.
        //
        // the descendant txid field refers to the directly **assumed canonical txC**
        let exp_reasons = vec![
            (
                "txA",
                CanonicalReason::Assumed {
                    descendant: Some(txid_c),
                },
            ),
            (
                "txB",
                CanonicalReason::Assumed {
                    descendant: Some(txid_c),
                },
            ),
            ("txC", CanonicalReason::Assumed { descendant: None }),
        ];

        // build task & canonicalize
        let canonical_params = env.canonicalization_params;
        let canonical_task = env.tx_graph.canonical_task(chain_tip, canonical_params);
        let canonical_txs = local_chain.canonicalize(canonical_task);

        // assert canonical transactions
        let exp_canonical_txids: HashSet<Txid> = scenario
            .exp_canonical_txs
            .iter()
            .map(|tx_name| {
                *env.txid_to_name
                    .get(tx_name)
                    .expect("txid should exist for tx_name")
            })
            .collect::<HashSet<Txid>>();

        let canonical_txids = canonical_txs
            .txs()
            .map(|canonical_tx| canonical_tx.txid)
            .collect::<HashSet<Txid>>();

        assert_eq!(
            canonical_txids, exp_canonical_txids,
            "[{}] canonical transactions mismatch",
            scenario.name
        );

        // assert canonical reasons
        for (tx_name, exp_reason) in exp_reasons {
            let txid = env
                .txid_to_name
                .get(tx_name)
                .expect("txid should exist for tx_name");

            let canonical_reason = canonical_txs
                .txs()
                .find(|ctx| &ctx.txid == txid)
                .expect("expected txid should exist in canonical txs")
                .pos;

            assert_eq!(
                canonical_reason, exp_reason,
                "[{}] canonical reason mismatch for {}",
                scenario.name, tx_name
            )
        }

        let txid_b = *env.txid_to_name.get("txB").unwrap();

        // build the expected `ChainPosition` with specific txid's for transitively confirmed txs.
        //
        // in this scenario:
        //
        // txA: should be confirmed transitively by txB.
        // txB: should be confirmed, has a direct anchor(block5).
        // txC: should be unconfirmed, has been assumed canonical though has no direct anchors.
        let exp_positions = vec![
            (
                "txA",
                ChainPosition::Confirmed {
                    anchor: block_id!(5, "block5"),
                    transitively: Some(txid_b),
                },
            ),
            (
                "txB",
                ChainPosition::Confirmed {
                    anchor: block_id!(5, "block5"),
                    transitively: None,
                },
            ),
            (
                "txC",
                ChainPosition::Unconfirmed {
                    first_seen: None,
                    last_seen: None,
                },
            ),
        ];

        // build task & resolve positions
        let view_task = canonical_txs.view_task(&env.tx_graph);
        let canonical_view = local_chain.canonicalize(view_task);

        // assert final positions
        for (tx_name, exp_position) in exp_positions {
            let txid = *env
                .txid_to_name
                .get(tx_name)
                .expect("txid should exist for tx_name");

            let canonical_position = canonical_view
                .txs()
                .find(|ctx| ctx.txid == txid)
                .expect("expected txid should exist in canonical view")
                .pos;

            assert_eq!(
                canonical_position, exp_position,
                "[{}] canonical position mismatch for {}",
                scenario.name, tx_name
            );
        }
    }
}
