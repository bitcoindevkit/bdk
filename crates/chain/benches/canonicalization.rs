use bdk_chain::local_chain::LocalChain;
use bdk_chain::spk_txout::SpkTxOutIndex;
use bdk_chain::TxGraph;
use bdk_core::BlockId;
use bdk_testenv::tx_template::{init_graph, TxInTemplate, TxOutTemplate, TxTemplate};
use bdk_testenv::{block_id, hash, local_chain};
use bitcoin::Amount;
use criterion::{criterion_group, criterion_main, Criterion};
use std::borrow::Cow;

fn filter_chain_unspents(
    tx_graph: &TxGraph<BlockId>,
    spk_index: &SpkTxOutIndex<u32>,
    chain: &LocalChain,
    exp_txos: usize,
) {
    let utxos = tx_graph.filter_chain_unspents(
        chain,
        chain.tip().block_id(),
        spk_index.outpoints().clone(),
    );
    assert_eq!(utxos.count(), exp_txos);
}

fn filter_chain_txouts(
    tx_graph: &TxGraph<BlockId>,
    spk_index: &SpkTxOutIndex<u32>,
    chain: &LocalChain,
    exp_txos: usize,
) {
    let utxos =
        tx_graph.filter_chain_txouts(chain, chain.tip().block_id(), spk_index.outpoints().clone());
    assert_eq!(utxos.count(), exp_txos);
}

fn list_canonical_txs(tx_graph: &TxGraph<BlockId>, chain: &LocalChain, exp_txs: usize) {
    let txs = tx_graph.list_canonical_txs(chain, chain.tip().block_id());
    assert_eq!(txs.count(), exp_txs);
}

fn setup_many_conflicting_unconfirmed(
    tx_count: u32,
) -> (TxGraph<BlockId>, SpkTxOutIndex<u32>, LocalChain) {
    let chain = local_chain![(0, hash!("genesis")), (100, hash!("abcd"))];
    let mut templates = Vec::new();

    templates.push(TxTemplate {
        tx_name: Cow::Borrowed("ancestor_tx"),
        inputs: Cow::Borrowed(&[TxInTemplate::Bogus]),
        outputs: Cow::Owned(vec![TxOutTemplate::new(Amount::ONE_BTC.to_sat(), Some(0))]),
        anchors: Cow::Owned(vec![block_id!(100, "abcd")]),
        last_seen: None,
    });

    for i in 1..=tx_count {
        templates.push(TxTemplate {
            tx_name: format!("conflict_tx_{}", i).into(),
            inputs: Cow::Owned(vec![TxInTemplate::PrevTx("ancestor_tx".into(), 0)]),
            outputs: Cow::Owned(vec![TxOutTemplate::new(
                Amount::ONE_BTC.to_sat() - (i as u64 * 10),
                Some(1),
            )]),
            last_seen: Some(i as u64),
            ..Default::default()
        });
    }

    let (tx_graph, spk_index, _) = init_graph(templates);
    (tx_graph, spk_index, chain)
}

/// chain of unconfirmed transactions
fn setup_many_chained_unconfirmed(
    tx_chain_count: u32,
) -> (TxGraph<BlockId>, SpkTxOutIndex<u32>, LocalChain) {
    let chain = local_chain![(0, hash!("genesis"))];
    let mut templates = Vec::new();

    templates.push(TxTemplate {
        tx_name: Cow::Borrowed("ancestor_tx"),
        inputs: Cow::Borrowed(&[TxInTemplate::Bogus]),
        outputs: Cow::Owned(vec![TxOutTemplate::new(Amount::ONE_BTC.to_sat(), Some(0))]),
        anchors: Cow::Owned(vec![block_id!(100, "abcd")]),
        last_seen: None,
    });

    for i in 0..tx_chain_count {
        templates.push(TxTemplate {
            tx_name: format!("chain_tx_{}", i).into(),
            inputs: if i == 0 {
                Cow::Owned(vec![TxInTemplate::PrevTx("ancestor_tx".into(), 0)])
            } else {
                Cow::Owned(vec![TxInTemplate::PrevTx(
                    format!("chain_tx_{}", i - 1).into(),
                    0,
                )])
            },
            last_seen: Some(i as u64),
            ..Default::default()
        });
    }

    let (tx_graph, spk_index, _) = init_graph(templates);
    (tx_graph, spk_index, chain)
}

/// graph with nested conflicting transactions
fn setup_nested_conflicts(
    graph_depth: usize,
    conflicts_per_output: usize,
) -> (TxGraph<BlockId>, SpkTxOutIndex<u32>, LocalChain) {
    let chain = local_chain![(0, hash!("genesis"))];
    let mut templates = Vec::new();

    templates.push(TxTemplate {
        tx_name: "ancestor_tx".into(),
        inputs: Cow::Borrowed(&[TxInTemplate::Bogus]),
        outputs: Cow::Owned(vec![TxOutTemplate::new(Amount::ONE_BTC.to_sat(), Some(0))]),
        anchors: Cow::Owned(vec![block_id!(100, "abcd")]),
        last_seen: None,
    });

    let mut previous_outputs = vec!["ancestor_tx".to_string()];

    for depth in 1..graph_depth {
        let mut next_outputs = Vec::new();

        for (parent_index, previous_output_name) in previous_outputs.drain(..).enumerate() {
            for conflict_i in 1..=conflicts_per_output {
                let tx_name = format!(
                    "depth_{}_parent_{}_conflict_{}",
                    depth, parent_index, conflict_i
                );

                let last_seen = depth as u64 * conflict_i as u64;

                templates.push(TxTemplate {
                    tx_name: tx_name.clone().into(),
                    inputs: Cow::Owned(vec![TxInTemplate::PrevTx(
                        previous_output_name.clone().into(),
                        0,
                    )]),
                    outputs: Cow::Owned(vec![TxOutTemplate::new(
                        Amount::ONE_BTC.to_sat() - (depth as u64 * 200 - conflict_i as u64),
                        Some(0),
                    )]),
                    last_seen: Some(last_seen),
                    ..Default::default()
                });

                next_outputs.push(tx_name);
            }
        }

        previous_outputs = next_outputs;
    }

    let (tx_graph, spk_index, _) = init_graph(templates);
    (tx_graph, spk_index, chain)
}

/// Benchmark for many conflicting unconfirmed transactions
fn bench_many_conflicting_unconfirmed(c: &mut Criterion) {
    const CONFLICTING_TX_COUNT: u32 = 2100;

    let (tx_graph, spk_index, chain) = setup_many_conflicting_unconfirmed(CONFLICTING_TX_COUNT);

    c.bench_function("many_conflicting_unconfirmed::list_canonical_txs", {
        let tx_graph = tx_graph.clone();
        let chain = chain.clone();
        move |b| {
            b.iter(|| list_canonical_txs(&tx_graph, &chain, 2));
        }
    });

    c.bench_function("many_conflicting_unconfirmed::filter_chain_txouts", {
        let tx_graph = tx_graph.clone();
        let spk_index = spk_index.clone();
        let chain = chain.clone();
        move |b| {
            b.iter(|| filter_chain_txouts(&tx_graph, &spk_index, &chain, 2));
        }
    });

    c.bench_function(
        "many_conflicting_unconfirmed::filter_chain_unspents",
        move |b| {
            b.iter(|| {
                filter_chain_unspents(&tx_graph, &spk_index, &chain, 1);
            });
        },
    );
}

/// Benchmark for many chained unconfirmed transactions
pub fn bench_many_chained_unconfirmed(c: &mut Criterion) {
    const TX_CHAIN_COUNT: u32 = 2100;

    let (tx_graph, spk_index, chain) = setup_many_chained_unconfirmed(TX_CHAIN_COUNT);

    c.bench_function("many_chained_unconfirmed::list_canonical_txs", {
        let tx_graph = tx_graph.clone();
        let chain = chain.clone();
        move |b| {
            b.iter(|| {
                list_canonical_txs(&tx_graph, &chain, (TX_CHAIN_COUNT + 1).try_into().unwrap());
            });
        }
    });

    c.bench_function("many_chained_unconfirmed::filter_chain_txouts", {
        let tx_graph = tx_graph.clone();
        let chain = chain.clone();
        let spk_index = spk_index.clone();
        move |b| {
            b.iter(|| {
                filter_chain_txouts(&tx_graph, &spk_index, &chain, 1);
            });
        }
    });

    c.bench_function("many_chained_unconfirmed::filter_chain_unspents", {
        move |b| {
            b.iter(|| {
                filter_chain_unspents(&tx_graph, &spk_index, &chain, 0);
            });
        }
    });
}

/// Benchmark for nested conflicts
pub fn bench_nested_conflicts(c: &mut Criterion) {
    const CONFLICTS_PER_OUTPUT: usize = 3;
    const GRAPH_DEPTH: usize = 7;

    let (tx_graph, spk_index, chain) = setup_nested_conflicts(GRAPH_DEPTH, CONFLICTS_PER_OUTPUT);

    c.bench_function("nested_conflicts_unconfirmed::list_canonical_txs", {
        let tx_graph = tx_graph.clone();
        let chain = chain.clone();
        move |b| {
            b.iter(|| {
                list_canonical_txs(&tx_graph, &chain, GRAPH_DEPTH);
            });
        }
    });

    c.bench_function("nested_conflicts_unconfirmed::filter_chain_txouts", {
        let tx_graph = tx_graph.clone();
        let chain = chain.clone();
        let spk_index = spk_index.clone();
        move |b| {
            b.iter(|| {
                filter_chain_txouts(&tx_graph, &spk_index, &chain, GRAPH_DEPTH);
            });
        }
    });

    c.bench_function("nested_conflicts_unconfirmed::filter_chain_unspents", {
        move |b| {
            b.iter(|| {
                filter_chain_unspents(&tx_graph, &spk_index, &chain, 1);
            });
        }
    });
}

criterion_group!(
    benches,
    bench_many_conflicting_unconfirmed,
    bench_many_chained_unconfirmed,
    bench_nested_conflicts,
);
criterion_main!(benches);
