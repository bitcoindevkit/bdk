#![allow(clippy::print_stdout)]
use std::time::Instant;

use anyhow::Context;
use bdk_bitcoind_rpc::bip158::{Event, EventInner, FilterIter};
use bdk_chain::bitcoin::{constants::genesis_block, secp256k1::Secp256k1, Network};
use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
use bdk_chain::local_chain::LocalChain;
use bdk_chain::miniscript::Descriptor;
use bdk_chain::{BlockId, ConfirmationBlockTime, IndexedTxGraph, SpkIterator};
use bdk_testenv::anyhow;
use bitcoin::Address;

// This example shows how BDK chain and tx-graph structures are updated using compact
// filters syncing. Assumes a connection can be made to a bitcoin node via environment
// variables `RPC_URL` and `RPC_COOKIE`.

// Usage: `cargo run -p bdk_bitcoind_rpc --example filter_iter`

const EXTERNAL: &str = "tr([7d94197e]tprv8ZgxMBicQKsPe1chHGzaa84k1inY2nAXUL8iPSyWESPrEst4E5oCFXhPATqj5fvw34LDknJz7rtXyEC4fKoXryUdc9q87pTTzfQyv61cKdE/86'/1'/0'/0/*)#uswl2jj7";
const INTERNAL: &str = "tr([7d94197e]tprv8ZgxMBicQKsPe1chHGzaa84k1inY2nAXUL8iPSyWESPrEst4E5oCFXhPATqj5fvw34LDknJz7rtXyEC4fKoXryUdc9q87pTTzfQyv61cKdE/86'/1'/0'/1/*)#dyt7h8zx";
const SPK_COUNT: u32 = 25;
const NETWORK: Network = Network::Signet;

const START_HEIGHT: u32 = 170_000;
const START_HASH: &str = "00000041c812a89f084f633e4cf47e819a2f6b1c0a15162355a930410522c99d";

fn main() -> anyhow::Result<()> {
    // Setup receiving chain and graph structures.
    let secp = Secp256k1::new();
    let (descriptor, _) = Descriptor::parse_descriptor(&secp, EXTERNAL)?;
    let (change_descriptor, _) = Descriptor::parse_descriptor(&secp, INTERNAL)?;
    let (mut chain, _) = LocalChain::from_genesis_hash(genesis_block(NETWORK).block_hash());

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<&str>>::new({
        let mut index = KeychainTxOutIndex::default();
        index.insert_descriptor("external", descriptor.clone())?;
        index.insert_descriptor("internal", change_descriptor.clone())?;
        index
    });

    // Assume a minimum birthday height
    let block = BlockId {
        height: START_HEIGHT,
        hash: START_HASH.parse()?,
    };
    let _ = chain.insert_block(block)?;

    // Configure RPC client
    let url = std::env::var("RPC_URL").context("must set RPC_URL")?;
    let cookie = std::env::var("RPC_COOKIE").context("must set RPC_COOKIE")?;
    let rpc_client =
        bitcoincore_rpc::Client::new(&url, bitcoincore_rpc::Auth::CookieFile(cookie.into()))?;

    // Initialize block emitter
    let cp = chain.tip();
    let start_height = cp.height();
    let mut emitter = FilterIter::new_with_checkpoint(&rpc_client, cp);
    for (_, desc) in graph.index.keychains() {
        let spks = SpkIterator::new_with_range(desc, 0..SPK_COUNT).map(|(_, spk)| spk);
        emitter.add_spks(spks);
    }

    let start = Instant::now();

    // Sync
    if let Some(tip) = emitter.get_tip()? {
        let blocks_to_scan = tip.height - start_height;

        for event in emitter.by_ref() {
            let event = event?;
            let curr = event.height();
            // apply relevant blocks
            if let Event::Block(EventInner { height, ref block }) = event {
                let _ = graph.apply_block_relevant(block, height);
                println!("Matched block {curr}");
            }
            if curr % 1000 == 0 {
                let progress = (curr - start_height) as f32 / blocks_to_scan as f32;
                println!("[{:.2}%]", progress * 100.0);
            }
        }
        // update chain
        if let Some(cp) = emitter.chain_update() {
            let _ = chain.apply_update(cp)?;
        }
    }

    println!("\ntook: {}s", start.elapsed().as_secs());
    println!("Local tip: {}", chain.tip().height());
    let unspent: Vec<_> = graph
        .graph()
        .filter_chain_unspents(
            &chain,
            chain.tip().block_id(),
            Default::default(),
            graph.index.outpoints().clone(),
        )
        .collect();
    if !unspent.is_empty() {
        println!("\nUnspent");
        for (index, utxo) in unspent {
            // (k, index) | value | outpoint |
            println!("{:?} | {} | {}", index, utxo.txout.value, utxo.outpoint);
        }
    }

    let unused_spk = graph.index.reveal_next_spk("external").unwrap().0 .1;
    let unused_address = Address::from_script(&unused_spk, NETWORK)?;
    println!("Next external address: {unused_address}");

    Ok(())
}
