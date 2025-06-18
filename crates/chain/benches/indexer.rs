use bdk_chain::{
    keychain_txout::{InsertDescriptorError, KeychainTxOutIndex},
    local_chain::LocalChain,
    CanonicalizationParams, IndexedTxGraph,
};
use bdk_core::{BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate};
use bitcoin::{
    absolute, constants, hashes::Hash, key::Secp256k1, transaction, Amount, BlockHash, Network,
    Transaction, TxIn, TxOut,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use miniscript::Descriptor;
use std::sync::Arc;

type Keychain = ();
type KeychainTxGraph = IndexedTxGraph<ConfirmationBlockTime, KeychainTxOutIndex<Keychain>>;

const DESC: &str = "tr([ab28dc00/86h/1h/0h]tpubDCdDtzAMZZrkwKBxwNcGCqe4FRydeD9rfMisoi7qLdraG79YohRfPW4YgdKQhpgASdvh612xXNY5xYzoqnyCgPbkpK4LSVcH5Xv4cK7johH/0/*)";
const LOOKAHEAD: u32 = 10;
const LAST_REVEALED: u32 = 500;
const TX_CT: u32 = 21;
const USE_SPK_CACHE: bool = true;
const AMOUNT: Amount = Amount::from_sat(1_000);

fn new_tx(lt: u32) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::from_consensus(lt),
        input: vec![],
        output: vec![TxOut::NULL],
    }
}

fn genesis_block_id() -> BlockId {
    BlockId {
        height: 0,
        hash: constants::genesis_block(Network::Regtest).block_hash(),
    }
}

fn tip_block_id() -> BlockId {
    BlockId {
        height: 100,
        hash: BlockHash::all_zeros(),
    }
}

fn setup<F: Fn(&mut KeychainTxGraph, &LocalChain)>(f: F) -> (KeychainTxGraph, LocalChain) {
    let desc = Descriptor::parse_descriptor(&Secp256k1::new(), DESC)
        .unwrap()
        .0;

    let cp = CheckPoint::from_block_ids([genesis_block_id(), tip_block_id()]).unwrap();
    let chain = LocalChain::from_tip(cp).unwrap();

    let mut index = KeychainTxOutIndex::new(LOOKAHEAD, USE_SPK_CACHE);
    index.insert_descriptor((), desc).unwrap();
    let mut tx_graph = KeychainTxGraph::new(index);

    f(&mut tx_graph, &chain);

    (tx_graph, chain)
}

/// Bench performance of recovering `KeychainTxOutIndex` from changeset.
fn do_bench(indexed_tx_graph: &KeychainTxGraph, chain: &LocalChain) {
    let desc = indexed_tx_graph.index.get_descriptor(()).unwrap();
    let changeset = indexed_tx_graph.initial_changeset();

    // Now recover
    let (graph, _cs) =
        KeychainTxGraph::from_changeset(changeset, |cs| -> Result<_, InsertDescriptorError<_>> {
            let mut index = KeychainTxOutIndex::from_changeset(LOOKAHEAD, USE_SPK_CACHE, cs);
            let _ = index.insert_descriptor((), desc.clone())?;
            Ok(index)
        })
        .unwrap();

    // Check balance
    let chain_tip = chain.tip().block_id();
    let op = graph.index.outpoints().clone();
    let bal = graph.graph().balance(
        chain,
        chain_tip,
        CanonicalizationParams::default(),
        op,
        |_, _| false,
    );
    assert_eq!(bal.total(), AMOUNT * TX_CT as u64);
}

pub fn reindex_tx_graph(c: &mut Criterion) {
    let (graph, chain) = black_box(setup(|graph, _chain| {
        // Add relevant txs to graph
        for i in 0..TX_CT {
            let script_pubkey = graph.index.reveal_next_spk(()).unwrap().0 .1;
            let tx = Transaction {
                input: vec![TxIn::default()],
                output: vec![TxOut {
                    script_pubkey,
                    value: AMOUNT,
                }],
                ..new_tx(i)
            };
            let txid = tx.compute_txid();
            let mut update = TxUpdate::default();
            update.seen_ats = [(txid, i as u64)].into();
            update.txs = vec![Arc::new(tx)];
            let _ = graph.apply_update(update);
        }
        // Reveal some SPKs
        let _ = graph.index.reveal_to_target((), LAST_REVEALED);
    }));

    c.bench_function("reindex_tx_graph", {
        move |b| b.iter(|| do_bench(&graph, &chain))
    });
}

criterion_group!(benches, reindex_tx_graph);
criterion_main!(benches);
