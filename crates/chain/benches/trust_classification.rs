//! Benchmarks for `classify_outpoints` (the classifier `balance` folds over).
//!
//! Each group builds a synthetic canonical graph and times classifying every UTXO across a range
//! of sizes. What we care about is how the cost grows with the unconfirmed ancestry, so the
//! confirmed part is kept minimal and everything interesting lives in the mempool.

use std::collections::BTreeMap;

use bdk_chain::{
    local_chain::LocalChain, CanonicalView, ChainPosition, ConfirmationBlockTime, TxGraph,
};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{Amount, BlockHash, OutPoint, Script, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

// Minimal 2-block chain. Confirmed txs just anchor at height 1.
fn make_chain() -> LocalChain {
    let blocks: BTreeMap<u32, BlockHash> = [(0, hash!("genesis")), (1, hash!("b1"))]
        .into_iter()
        .collect();
    LocalChain::from_blocks(blocks).unwrap()
}

// The two predicates `classify_outpoints` takes, faking what a wallet would pass:
// `does_taint` = the tx pulls in coins that aren't ours, `is_settled` = the tx is confirmed.
#[allow(clippy::type_complexity)]
fn does_taint_and_is_settled<'a>(
    view: &'a CanonicalView<ConfirmationBlockTime>,
    owned: &ScriptBuf,
) -> (
    impl FnMut(&bdk_chain::CanonicalTx<ChainPosition<ConfirmationBlockTime>>) -> bool + 'a,
    impl Fn(&ChainPosition<ConfirmationBlockTime>) -> bool,
) {
    let owned = owned.clone();
    let is_mine = move |spk: &Script| spk == owned.as_script();
    let does_taint = move |c_tx: &bdk_chain::CanonicalTx<ChainPosition<ConfirmationBlockTime>>| {
        c_tx.tx
            .input
            .iter()
            .any(|txin| match view.tx(txin.previous_output.txid) {
                Some(prev) => {
                    !is_mine(&prev.tx.output[txin.previous_output.vout as usize].script_pubkey)
                }
                // input from a tx we don't have, treat it as foreign
                None => true,
            })
    };
    let is_settled =
        |pos: &ChainPosition<ConfirmationBlockTime>| matches!(pos, ChainPosition::Confirmed { .. });
    (does_taint, is_settled)
}

// Per-UTXO memoized classification (`classify_outpoints`).
fn run_classify(
    utxo_txids: &[Txid],
    view: &CanonicalView<ConfirmationBlockTime>,
    owned: &ScriptBuf,
) {
    let outpoints = utxo_txids.iter().map(|&txid| OutPoint::new(txid, 0));
    let (does_taint, is_settled) = does_taint_and_is_settled(view, owned);
    for item in view.classify_outpoints(outpoints, does_taint, is_settled) {
        std::hint::black_box(item);
    }
}

// One confirmed owned root with `width` outputs, each starting a `depth`-deep chain of unconfirmed
// owned txs. Nothing taints. Walking back, every chain converges on the shared root.
fn setup_fan_in(
    width: usize,
    depth: usize,
) -> (
    LocalChain,
    TxGraph<ConfirmationBlockTime>,
    Vec<Txid>,
    ScriptBuf,
) {
    let chain = make_chain();
    let mut tx_graph = TxGraph::default();
    let owned = ScriptBuf::from(vec![1u8]);

    let root_tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("external"), 0),
            ..Default::default()
        }],
        output: (0..width)
            .map(|_| TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: owned.clone(),
            })
            .collect(),
        ..new_tx(0)
    };
    let root_txid = root_tx.compute_txid();
    let _ = tx_graph.insert_tx(root_tx);
    let _ = tx_graph.insert_anchor(
        root_txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 1,
        },
    );

    let mut utxo_txids = vec![];
    for chain_i in 0..width {
        let mut prev = OutPoint::new(root_txid, chain_i as u32);
        for layer in 0..depth {
            let tx = Transaction {
                input: vec![TxIn {
                    previous_output: prev,
                    ..Default::default()
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(90_000),
                    script_pubkey: owned.clone(),
                }],
                ..new_tx((chain_i * depth + layer + 1) as u32)
            };
            let txid = tx.compute_txid();
            let _ = tx_graph.insert_tx(tx);
            let _ = tx_graph.insert_seen_at(txid, 100);
            prev = OutPoint::new(txid, 0);
            if layer == depth - 1 {
                utxo_txids.push(txid);
            }
        }
    }
    (chain, tx_graph, utxo_txids, owned)
}

// Like fan_in but each chain gets its own confirmed root, so there is no shared ancestor.
// Nothing taints.
fn setup_disjoint(
    width: usize,
    depth: usize,
) -> (
    LocalChain,
    TxGraph<ConfirmationBlockTime>,
    Vec<Txid>,
    ScriptBuf,
) {
    let chain = make_chain();
    let mut tx_graph = TxGraph::default();
    let owned = ScriptBuf::from(vec![1u8]);

    let mut utxo_txids = vec![];
    for chain_i in 0..width {
        let root_tx = Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(hash!("external"), chain_i as u32),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: owned.clone(),
            }],
            ..new_tx(chain_i as u32)
        };
        let root_txid = root_tx.compute_txid();
        let _ = tx_graph.insert_tx(root_tx);
        let _ = tx_graph.insert_anchor(
            root_txid,
            ConfirmationBlockTime {
                block_id: chain.get(1).unwrap().block_id(),
                confirmation_time: 1,
            },
        );

        let mut prev = OutPoint::new(root_txid, 0);
        for layer in 0..depth {
            let tx = Transaction {
                input: vec![TxIn {
                    previous_output: prev,
                    ..Default::default()
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(90_000),
                    script_pubkey: owned.clone(),
                }],
                ..new_tx((width + chain_i * depth + layer) as u32)
            };
            let txid = tx.compute_txid();
            let _ = tx_graph.insert_tx(tx);
            let _ = tx_graph.insert_seen_at(txid, 100);
            prev = OutPoint::new(txid, 0);
            if layer == depth - 1 {
                utxo_txids.push(txid);
            }
        }
    }
    (chain, tx_graph, utxo_txids, owned)
}

// An unconfirmed root paying foreign scripts. Each chain's first tx spends it, so it taints, and
// the taint carries down the chain. Every UTXO ends up untrusted.
fn setup_untrusted_fan_in(
    width: usize,
    depth: usize,
) -> (
    LocalChain,
    TxGraph<ConfirmationBlockTime>,
    Vec<Txid>,
    ScriptBuf,
) {
    let chain = make_chain();
    let mut tx_graph = TxGraph::default();
    let owned = ScriptBuf::from(vec![1u8]);
    let foreign = ScriptBuf::from(vec![2u8]); // not owned -> untrusted

    // Unconfirmed root with a foreign input
    let root_tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("external"), 0),
            ..Default::default()
        }],
        output: (0..width)
            .map(|_| TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: foreign.clone(),
            })
            .collect(),
        ..new_tx(0)
    };
    let root_txid = root_tx.compute_txid();
    let _ = tx_graph.insert_tx(root_tx);
    let _ = tx_graph.insert_seen_at(root_txid, 50);

    let mut utxo_txids = vec![];
    for chain_i in 0..width {
        let mut prev = OutPoint::new(root_txid, chain_i as u32);
        for layer in 0..depth {
            let tx = Transaction {
                input: vec![TxIn {
                    previous_output: prev,
                    ..Default::default()
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(90_000),
                    script_pubkey: owned.clone(),
                }],
                ..new_tx((chain_i * depth + layer + 1) as u32)
            };
            let txid = tx.compute_txid();
            let _ = tx_graph.insert_tx(tx);
            let _ = tx_graph.insert_seen_at(txid, 100);
            prev = OutPoint::new(txid, 0);
            if layer == depth - 1 {
                utxo_txids.push(txid);
            }
        }
    }
    (chain, tx_graph, utxo_txids, owned)
}

// `shared` confirmed base txs, and `utxos` outputs that each spend  all of the base txs (many
// inputs converging on the same ancestors). Given importance on shared ancestry and nothing taints.
fn setup_diamond(
    shared: usize,
    utxos: usize,
) -> (
    LocalChain,
    TxGraph<ConfirmationBlockTime>,
    Vec<Txid>,
    ScriptBuf,
) {
    let chain = make_chain();
    let mut tx_graph = TxGraph::default();
    let owned = ScriptBuf::from(vec![1u8]);

    let root_tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("external"), 0),
            ..Default::default()
        }],
        output: (0..shared)
            .map(|_| TxOut {
                value: Amount::from_sat(1_000_000),
                script_pubkey: owned.clone(),
            })
            .collect(),
        ..new_tx(0)
    };
    let root_txid = root_tx.compute_txid();
    let _ = tx_graph.insert_tx(root_tx);
    let _ = tx_graph.insert_anchor(
        root_txid,
        ConfirmationBlockTime {
            block_id: chain.get(1).unwrap().block_id(),
            confirmation_time: 1,
        },
    );

    let mut base_txids = vec![];
    for i in 0..shared {
        let base_tx = Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(root_txid, i as u32),
                ..Default::default()
            }],
            output: (0..utxos)
                .map(|_| TxOut {
                    value: Amount::from_sat(90_000),
                    script_pubkey: owned.clone(),
                })
                .collect(),
            ..new_tx((i + 1) as u32)
        };
        let base_txid = base_tx.compute_txid();
        let _ = tx_graph.insert_tx(base_tx);
        let _ = tx_graph.insert_seen_at(base_txid, 50);
        base_txids.push(base_txid);
    }

    let mut utxo_txids = vec![];
    for j in 0..utxos {
        let utxo_tx = Transaction {
            input: base_txids
                .iter()
                .map(|&base_txid| TxIn {
                    previous_output: OutPoint::new(base_txid, j as u32),
                    ..Default::default()
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: owned.clone(),
            }],
            ..new_tx((shared + 1 + j) as u32)
        };
        let utxo_txid = utxo_tx.compute_txid();
        let _ = tx_graph.insert_tx(utxo_tx);
        let _ = tx_graph.insert_seen_at(utxo_txid, 100);
        utxo_txids.push(utxo_txid);
    }
    (chain, tx_graph, utxo_txids, owned)
}

// Each UTXO sits below a short owned tail, a single tainting tx, and a deep unconfirmed foreign
// ancestry above it. The taint is found right above the UTXO, so the classifier can stop there
// instead of walking the whole foreign chain. Every UTXO ends up untrusted.
fn setup_tainted_midchain(
    width: usize,
    depth: usize,
) -> (
    LocalChain,
    TxGraph<ConfirmationBlockTime>,
    Vec<Txid>,
    ScriptBuf,
) {
    let chain = make_chain();
    let mut tx_graph = TxGraph::default();
    let owned = ScriptBuf::from(vec![1u8]);
    let foreign = ScriptBuf::from(vec![2u8]);

    let mut n: u32 = 0;
    let mut utxo_txids = vec![];
    for chain_i in 0..width {
        let mut prev = OutPoint::new(hash!("external"), chain_i as u32);
        for _ in 0..depth {
            let tx = Transaction {
                input: vec![TxIn {
                    previous_output: prev,
                    ..Default::default()
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(100_000),
                    script_pubkey: foreign.clone(),
                }],
                ..new_tx(n)
            };
            n += 1;
            let txid = tx.compute_txid();
            let _ = tx_graph.insert_tx(tx);
            let _ = tx_graph.insert_seen_at(txid, 100);
            prev = OutPoint::new(txid, 0);
        }
        let taint_tx = Transaction {
            input: vec![TxIn {
                previous_output: prev,
                ..Default::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(90_000),
                script_pubkey: owned.clone(),
            }],
            ..new_tx(n)
        };
        n += 1;
        let taint_txid = taint_tx.compute_txid();
        let _ = tx_graph.insert_tx(taint_tx);
        let _ = tx_graph.insert_seen_at(taint_txid, 100);
        let utxo_tx = Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(taint_txid, 0),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(80_000),
                script_pubkey: owned.clone(),
            }],
            ..new_tx(n)
        };
        n += 1;
        let utxo_txid = utxo_tx.compute_txid();
        let _ = tx_graph.insert_tx(utxo_tx);
        let _ = tx_graph.insert_seen_at(utxo_txid, 100);
        utxo_txids.push(utxo_txid);
    }
    (chain, tx_graph, utxo_txids, owned)
}

fn bench_fan_in(c: &mut Criterion) {
    let mut group = c.benchmark_group("fan_in");
    for (width, depth) in [(10, 3), (50, 5), (100, 10), (500, 20)] {
        let label = format!("{width}w×{depth}d");
        let (chain, tx_graph, utxo_txids, owned) = setup_fan_in(width, depth);
        let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
        group.bench_with_input(BenchmarkId::new("classify", &label), &(), |b, _| {
            b.iter(|| run_classify(&utxo_txids, &view, &owned));
        });
    }
    group.finish();
}

fn bench_disjoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("disjoint");
    for (width, depth) in [(10, 3), (50, 5), (100, 10), (500, 20)] {
        let label = format!("{width}w×{depth}d");
        let (chain, tx_graph, utxo_txids, owned) = setup_disjoint(width, depth);
        let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
        group.bench_with_input(BenchmarkId::new("classify", &label), &(), |b, _| {
            b.iter(|| run_classify(&utxo_txids, &view, &owned));
        });
    }
    group.finish();
}

fn bench_untrusted_fan_in(c: &mut Criterion) {
    let mut group = c.benchmark_group("untrusted_fan_in");
    for (width, depth) in [(10, 3), (50, 5), (100, 10), (500, 20)] {
        let label = format!("{width}w×{depth}d");
        let (chain, tx_graph, utxo_txids, owned) = setup_untrusted_fan_in(width, depth);
        let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
        group.bench_with_input(BenchmarkId::new("classify", &label), &(), |b, _| {
            b.iter(|| run_classify(&utxo_txids, &view, &owned));
        });
    }
    group.finish();
}

fn bench_diamond(c: &mut Criterion) {
    let mut group = c.benchmark_group("diamond");
    for (shared, utxos) in [(5, 20), (10, 50), (20, 100), (50, 500)] {
        let label = format!("{shared}s×{utxos}u");
        let (chain, tx_graph, utxo_txids, owned) = setup_diamond(shared, utxos);
        let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
        group.bench_with_input(BenchmarkId::new("classify", &label), &(), |b, _| {
            b.iter(|| run_classify(&utxo_txids, &view, &owned));
        });
    }
    group.finish();
}

fn bench_tainted_midchain(c: &mut Criterion) {
    let mut group = c.benchmark_group("tainted_midchain");
    for (width, depth) in [(10, 3), (50, 5), (100, 10), (500, 20)] {
        let label = format!("{width}w×{depth}d");
        let (chain, tx_graph, utxo_txids, owned) = setup_tainted_midchain(width, depth);
        let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), Default::default());
        group.bench_with_input(BenchmarkId::new("classify", &label), &(), |b, _| {
            b.iter(|| run_classify(&utxo_txids, &view, &owned));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_fan_in,
    bench_disjoint,
    bench_untrusted_fan_in,
    bench_diamond,
    bench_tainted_midchain
);
criterion_main!(benches);
