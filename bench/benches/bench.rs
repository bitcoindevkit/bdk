extern crate bdk_chain;
extern crate criterion;

use criterion::{criterion_group, criterion_main};

criterion_group!(benches,
    bdk_chain::tx_graph::bench::filter_chain_unspents,
    bdk_chain::tx_graph::bench::list_canonical_txs,
    bdk_chain::tx_graph::bench::nested_conflicts,
);
criterion_main!(benches);
