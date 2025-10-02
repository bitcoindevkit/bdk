use std::collections::HashSet;
use std::str::FromStr;
use std::time::Instant;

use anyhow::Context;
use bdk_chain::bitcoin::{secp256k1::Secp256k1, Network};
use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
use bdk_chain::miniscript::Descriptor;
use bdk_chain::{
    Anchor, BlockId, CanonicalizationParams, CanonicalizationTask, ChainQuery,
    ConfirmationBlockTime, IndexedTxGraph, SpkIterator,
};
use bip157::chain::{BlockHeaderChanges, ChainState};
use bip157::messages::Event;
use bip157::{error::FetchBlockError, Builder, Client, HeaderCheckpoint, Requester};
use tracing::{debug, error, info, warn};

// This example shows how to use Kyoto (BIP157/158) with ChainOracle
// to handle canonicalization without storing all chain data locally.

const EXTERNAL: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/0/*)";
const INTERNAL: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/1/*)";
const SPK_COUNT: u32 = 25;

const NETWORK: Network = Network::Signet;
const START_HEIGHT: u32 = 201_000;
const START_HASH: &str = "0000002238d05b522875f9edc4c9f418dd89ccfde7e4c305e8448a87a5dc71b7";

/// `KyotoOracle`` uses Kyoto's requester for on-demand chain queries
/// It doesn't implement `ChainOracle` trait since that's synchronous and we need async
pub struct KyotoOracle {
    /// Requester to fetch blocks on-demand
    requester: Requester,
    /// Current chain tip
    chain_tip: BlockId,
}

impl KyotoOracle {
    pub fn new(requester: Requester, chain_tip: BlockId) -> Self {
        Self {
            requester,
            chain_tip,
        }
    }

    /// Get the current chain tip
    pub fn get_chain_tip(&self) -> BlockId {
        self.chain_tip
    }

    /// Canonicalize a transaction graph using async on-demand queries to Kyoto
    pub async fn canonicalize<A: Anchor>(
        &self,
        mut task: CanonicalizationTask<'_, A>,
    ) -> bdk_chain::CanonicalView<A> {
        // Process all queries from the task
        while let Some(request) = task.next_query() {
            // Check each block_id against the chain to find the best one
            let mut best_block = None;

            for block_id in &request.block_ids {
                // Check if block is in chain by fetching it on-demand
                match self.is_block_in_chain(*block_id).await {
                    Ok(true) => {
                        best_block = Some(*block_id);
                        break; // Found a confirmed block
                    }
                    Ok(false) => continue, // Not in chain, check next
                    Err(_) => continue,    // Error fetching, skip this one
                }
            }

            task.resolve_query(best_block);
        }

        // Finish and return the canonical view
        task.finish()
    }

    /// Check if a block is in the chain by fetching it on-demand from Kyoto
    async fn is_block_in_chain(&self, block: BlockId) -> Result<bool, FetchBlockError> {
        // Check if the requested block height is within range
        if block.height > self.chain_tip.height {
            return Ok(false);
        }

        // Try to fetch the block by its hash
        // If it exists and the height matches, it's in the chain
        match self.requester.get_block(block.hash).await {
            Ok(indexed_block) => {
                // Verify the height matches what we expect
                Ok(indexed_block.height == block.height)
            }
            Err(FetchBlockError::UnknownHash) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Setup descriptors and graph
    let secp = Secp256k1::new();
    let (descriptor, _) = Descriptor::parse_descriptor(&secp, EXTERNAL)?;
    let (change_descriptor, _) = Descriptor::parse_descriptor(&secp, INTERNAL)?;

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<&str>>::new({
        let mut index = KeychainTxOutIndex::default();
        index.insert_descriptor("external", descriptor.clone())?;
        index.insert_descriptor("internal", change_descriptor.clone())?;
        index
    });

    // Collect scripts to watch
    let mut spks = HashSet::new();
    for (_, desc) in graph.index.keychains() {
        spks.extend(SpkIterator::new_with_range(desc, 0..SPK_COUNT).map(|(_, s)| s));
    }

    // Build Kyoto node with checkpoint
    let checkpoint = HeaderCheckpoint::new(
        START_HEIGHT,
        bitcoin::BlockHash::from_str(START_HASH).context("invalid checkpoint hash")?,
    );

    let builder = Builder::new(NETWORK);
    let (node, client) = builder
        .chain_state(ChainState::Checkpoint(checkpoint))
        .required_peers(1)
        .build();

    // Run the node in background
    tokio::task::spawn(async move { node.run().await });

    let Client {
        requester,
        mut info_rx,
        mut warn_rx,
        mut event_rx,
    } = client;

    let start = Instant::now();
    #[allow(unused_assignments)]
    let mut chain_tip = BlockId {
        height: 0,
        hash: bitcoin::constants::genesis_block(bitcoin::Network::Signet).block_hash(),
    };
    let mut matched_blocks_count = 0;

    info!("Starting sync with Kyoto...");

    // Event loop to process filters and apply matching blocks immediately
    #[allow(unused_assignments)]
    #[allow(clippy::incompatible_msrv)]
    loop {
        tokio::select! {
            info_msg = info_rx.recv() => {
                if let Some(info_msg) = info_msg {
                    info!("Kyoto: {}", info_msg);
                }
            }
            warn_msg = warn_rx.recv() => {
                if let Some(warn_msg) = warn_msg {
                    warn!("Kyoto: {}", warn_msg);
                }
            }
            event = event_rx.recv() => {
                if let Some(event) = event {
                    match event {
                        Event::IndexedFilter(filter) => {
                            let height = filter.height();
                            if filter.contains_any(spks.iter()) {
                                let hash = filter.block_hash();
                                info!("Matched filter at height {}", height);
                                match requester.get_block(hash).await {
                                    Ok(indexed_block) => {
                                        // Apply block immediately to the graph
                                        let _ = graph.apply_block_relevant(&indexed_block.block, indexed_block.height);
                                        matched_blocks_count += 1;
                                        debug!("Applied block at height {}", indexed_block.height);
                                    }
                                    Err(e) => {
                                        error!("Failed to fetch block {}: {}", hash, e);
                                    }
                                }
                            }
                        },
                        Event::ChainUpdate(changes) => {
                            match &changes {
                                BlockHeaderChanges::Connected(header) => {
                                    // Update chain tip on each new header
                                    chain_tip = BlockId {
                                        height: header.height,
                                        hash: header.block_hash(),
                                    };
                                    if header.height % 1000 == 0 {
                                        info!("Synced to height {}", header.height);
                                    }
                                }
                                BlockHeaderChanges::Reorganized { accepted, .. } => {
                                    // On reorg, update to the new tip (last in accepted)
                                    if let Some(header) = accepted.last() {
                                        chain_tip = BlockId {
                                            height: header.height,
                                            hash: header.block_hash(),
                                        };
                                        warn!("Reorg to height {}", header.height);
                                    }
                                }
                                BlockHeaderChanges::ForkAdded(_) => {
                                    // Ignore forks that are not on the main chain
                                    debug!("Fork detected, ignoring");
                                }
                            }
                        }
                        Event::FiltersSynced(sync_update) => {
                            let tip = sync_update.tip();
                            chain_tip = BlockId {
                                height: tip.height,
                                hash: tip.hash,
                            };
                            info!("Filters synced! Tip: height={}, hash={}", tip.height, tip.hash);
                            break;
                        }
                        _ => (),
                    }
                }
            }
        }
    }

    info!("Sync completed in {}s", start.elapsed().as_secs());
    info!("Found and applied {} matching blocks", matched_blocks_count);

    info!(
        "Chain tip: height={}, hash={}",
        chain_tip.height, chain_tip.hash
    );

    // Create KyotoOracle with requester for on-demand queries
    let oracle = KyotoOracle::new(requester.clone(), chain_tip);

    // Canonicalize TxGraph with KyotoOracle
    info!("Performing canonicalization using KyotoOracle...");
    let task = graph.canonicalization_task(chain_tip, CanonicalizationParams::default());
    let canonical_view = oracle.canonicalize(task).await;

    // Display unspent outputs
    let unspent: Vec<_> = canonical_view
        .filter_unspent_outpoints(graph.index.outpoints().clone())
        .collect();

    if !unspent.is_empty() {
        info!("Found {} unspent outputs:", unspent.len());
        for (index, utxo) in unspent {
            info!("{:?} | {} | {}", index, utxo.txout.value, utxo.outpoint);
        }
    } else {
        info!("No unspent outputs found");
    }

    // Verify all canonical transactions are confirmed
    for canon_tx in canonical_view.txs() {
        if !canon_tx.pos.is_confirmed() {
            error!("Canonical tx should be confirmed: {}", canon_tx.txid);
        }
    }

    let _ = requester.shutdown();
    info!("Shutdown complete");

    Ok(())
}
