#![allow(clippy::print_stdout)]
use bdk_bitcoind_rpc::bip158::{Event, EventInner, FilterIter};
use bdk_chain::bitcoin::{constants::genesis_block, secp256k1::Secp256k1, Network};
use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
use bdk_chain::local_chain::LocalChain;
use bdk_chain::miniscript::Descriptor;
use bdk_chain::{BlockId, ConfirmationBlockTime, IndexedTxGraph, SpkIterator};
use bdk_testenv::anyhow;

// This example shows how BDK chain and tx-graph structures are updated using compact filters syncing.
// assumes a local Signet node, and "RPC_COOKIE" set in environment.

// Usage: `cargo run -p bdk_bitcoind_rpc --example bip158`

const EXTERNAL: &str = "tr([83737d5e/86h/1h/0h]tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/0/*)";
const INTERNAL: &str = "tr([83737d5e/86h/1h/0h]tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/1/*)";
const SPK_COUNT: u32 = 10;
const NETWORK: Network = Network::Signet;

fn main() -> anyhow::Result<()> {
    // Setup receiving chain and graph structures.
    let secp = Secp256k1::new();
    let (descriptor, _) = Descriptor::parse_descriptor(&secp, EXTERNAL)?;
    let (change_descriptor, _) = Descriptor::parse_descriptor(&secp, INTERNAL)?;
    let (mut chain, _) = LocalChain::from_genesis_hash(genesis_block(NETWORK).block_hash());
    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<usize>>::new({
        let mut index = KeychainTxOutIndex::default();
        index.insert_descriptor(0, descriptor.clone())?;
        index.insert_descriptor(1, change_descriptor.clone())?;
        index
    });

    // Assume a minimum birthday height
    let block = BlockId {
        height: 205_000,
        hash: "0000002bd0f82f8c0c0f1e19128f84c938763641dba85c44bdb6aed1678d16cb".parse()?,
    };
    let _ = chain.insert_block(block)?;

    // Configure RPC client
    let rpc_client = bitcoincore_rpc::Client::new(
        "127.0.0.1:38332",
        bitcoincore_rpc::Auth::CookieFile(std::env::var("RPC_COOKIE")?.into()),
    )?;

    // Initialize block emitter
    let mut emitter = FilterIter::new_with_checkpoint(&rpc_client, chain.tip());
    for (_, desc) in graph.index.keychains() {
        let spks = SpkIterator::new_with_range(desc, 0..SPK_COUNT).map(|(_, spk)| spk);
        emitter.add_spks(spks);
    }

    // Sync
    if emitter.get_tip()?.is_some() {
        // apply relevant blocks
        for event in emitter.by_ref() {
            if let Event::Block(EventInner { height, block }) = event? {
                let _ = graph.apply_block_relevant(&block, height);
            }
        }
        // update chain
        if let Some(tip) = emitter.chain_update() {
            let _ = chain.apply_update(tip)?;
        }
    }

    println!("Local tip: {}", chain.tip().height());

    println!("Unspent");
    for (_, outpoint) in graph.index.outpoints() {
        println!("{outpoint}");
    }

    Ok(())
}
