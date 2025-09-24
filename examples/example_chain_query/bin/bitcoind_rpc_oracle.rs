#![allow(clippy::print_stdout, clippy::print_stderr)]
use std::time::Instant;

use anyhow::Context;
use bdk_chain::bitcoin::{bip158::BlockFilter, secp256k1::Secp256k1, Block, ScriptBuf};
use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
use bdk_chain::miniscript::Descriptor;
use bdk_chain::{
    Anchor, BlockId, CanonicalizationParams, CanonicalizationTask, ChainOracle, ChainQuery,
    ConfirmationBlockTime, IndexedTxGraph, SpkIterator,
};
use bitcoincore_rpc::json::GetBlockHeaderResult;
use bitcoincore_rpc::{Client, RpcApi};

// This example shows how to use a CoreOracle that implements ChainOracle trait
// to handle canonicalization with bitcoind RPC, without needing LocalChain.

const EXTERNAL: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/0/*)";
const INTERNAL: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/1/*)";
const SPK_COUNT: u32 = 25;

const START_HEIGHT: u32 = 205_000;

/// Error types for CoreOracle and FilterIterV2
#[derive(Debug)]
pub enum Error {
    /// RPC error
    Rpc(bitcoincore_rpc::Error),
    /// `bitcoin::bip158` error
    Bip158(bdk_chain::bitcoin::bip158::Error),
    /// Max reorg depth exceeded
    ReorgDepthExceeded,
    /// Error converting an integer
    TryFromInt(core::num::TryFromIntError),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(e) => write!(f, "{e}"),
            Self::Bip158(e) => write!(f, "{e}"),
            Self::ReorgDepthExceeded => write!(f, "maximum reorg depth exceeded"),
            Self::TryFromInt(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<bitcoincore_rpc::Error> for Error {
    fn from(e: bitcoincore_rpc::Error) -> Self {
        Self::Rpc(e)
    }
}

impl From<core::num::TryFromIntError> for Error {
    fn from(e: core::num::TryFromIntError) -> Self {
        Self::TryFromInt(e)
    }
}

impl From<bdk_chain::bitcoin::bip158::Error> for Error {
    fn from(e: bdk_chain::bitcoin::bip158::Error) -> Self {
        Self::Bip158(e)
    }
}

/// Whether the RPC error is a "not found" error (code: `-5`)
fn is_not_found(e: &bitcoincore_rpc::Error) -> bool {
    matches!(
        e,
        bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Rpc(e))
        if e.code == -5
    )
}

/// CoreOracle implements ChainOracle using bitcoind RPC
pub struct CoreOracle {
    client: Client,
}

impl CoreOracle {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Canonicalize a transaction graph using this oracle
    pub fn canonicalize<A: Anchor>(
        &self,
        mut task: CanonicalizationTask<'_, A>,
        chain_tip: BlockId,
    ) -> bdk_chain::CanonicalView<A> {
        // Process all queries from the task
        while let Some(request) = task.next_query() {
            // Check each block_id against the chain to find the best one
            let mut best_block = None;

            for block_id in &request.block_ids {
                // Check if block is in chain
                match self.is_block_in_chain(*block_id, chain_tip) {
                    Ok(Some(true)) => {
                        best_block = Some(*block_id);
                        break; // Found a confirmed block
                    }
                    _ => continue, // Not confirmed or error, check next
                }
            }

            task.resolve_query(best_block);
        }

        // Finish and return the canonical view
        task.finish()
    }
}

impl ChainOracle for CoreOracle {
    type Error = Error;

    fn is_block_in_chain(
        &self,
        block: BlockId,
        chain_tip: BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        // Check if the requested block height is within range
        if block.height > chain_tip.height {
            return Ok(Some(false));
        }

        // Get the block hash at the requested height
        match self.client.get_block_hash(block.height as u64) {
            Ok(hash_at_height) => Ok(Some(hash_at_height == block.hash)),
            Err(e) if is_not_found(&e) => Ok(Some(false)),
            Err(_) => Ok(None), // Can't determine, return None
        }
    }

    fn get_chain_tip(&self) -> Result<BlockId, Self::Error> {
        let height = self.client.get_block_count()? as u32;
        let hash = self.client.get_block_hash(height as u64)?;
        Ok(BlockId { height, hash })
    }
}

/// FilterIterV2: Similar to FilterIter but doesn't manage CheckPoints
pub struct FilterIterV2<'a> {
    client: &'a Client,
    spks: Vec<ScriptBuf>,
    current_height: u32,
    header: Option<GetBlockHeaderResult>,
}

impl<'a> FilterIterV2<'a> {
    pub fn new(
        client: &'a Client,
        start_height: u32,
        spks: impl IntoIterator<Item = ScriptBuf>,
    ) -> Self {
        Self {
            client,
            spks: spks.into_iter().collect(),
            current_height: start_height,
            header: None,
        }
    }

    /// Find the starting point for iteration
    fn find_base(&self) -> Result<GetBlockHeaderResult, Error> {
        let hash = self.client.get_block_hash(self.current_height as u64)?;
        Ok(self.client.get_block_header_info(&hash)?)
    }
}

/// Event returned by FilterIterV2 - contains a block that matches the filter
#[derive(Debug, Clone)]
pub struct EventV2 {
    pub block: Option<Block>,
    pub height: u32,
}

impl Iterator for FilterIterV2<'_> {
    type Item = Result<EventV2, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let result = (|| -> Result<Option<EventV2>, Error> {
            let header = match self.header.take() {
                Some(header) => header,
                None => self.find_base()?,
            };

            let next_hash = match header.next_block_hash {
                Some(hash) => hash,
                None => return Ok(None), // Reached chain tip
            };

            let mut next_header = self.client.get_block_header_info(&next_hash)?;

            // Handle reorgs
            while next_header.confirmations < 0 {
                let prev_hash = next_header
                    .previous_block_hash
                    .ok_or(Error::ReorgDepthExceeded)?;
                next_header = self.client.get_block_header_info(&prev_hash)?;
            }

            let height = next_header.height.try_into()?;
            let hash = next_header.hash;

            // Check if block matches our filters
            let mut block = None;
            let filter = BlockFilter::new(self.client.get_block_filter(&hash)?.filter.as_slice());

            if filter.match_any(&hash, self.spks.iter().map(ScriptBuf::as_ref))? {
                block = Some(self.client.get_block(&hash)?);
            }

            // Update state
            self.current_height = height;
            self.header = Some(next_header);

            Ok(Some(EventV2 { block, height }))
        })();

        result.transpose()
    }
}

fn main() -> anyhow::Result<()> {
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

    // Configure RPC client
    let url = std::env::var("RPC_URL").context("must set RPC_URL")?;
    let cookie = std::env::var("RPC_COOKIE").context("must set RPC_COOKIE")?;
    let rpc_client = Client::new(&url, bitcoincore_rpc::Auth::CookieFile(cookie.into()))?;

    // Initialize `FilterIter`
    let mut spks = vec![];
    for (_, desc) in graph.index.keychains() {
        spks.extend(SpkIterator::new_with_range(desc, 0..SPK_COUNT).map(|(_, s)| s));
    }
    let iter = FilterIterV2::new(&rpc_client, START_HEIGHT, spks);

    let start = Instant::now();

    for res in iter {
        let event = res?;

        if let Some(block) = event.block {
            let _ = graph.apply_block_relevant(&block, event.height);
            println!("Matched block {}", event.height);
        }
    }

    println!("\ntook: {}s", start.elapsed().as_secs());

    // Create `CoreOracle`
    let oracle = CoreOracle::new(rpc_client);

    // Get current chain tip from `CoreOracle`
    let chain_tip = oracle.get_chain_tip()?;
    println!(
        "chain tip: height={}, hash={}",
        chain_tip.height, chain_tip.hash
    );

    // Canonicalize TxGraph with `CoreCoracle`
    println!("\nPerforming canonicalization using CoreOracle...");
    let task = graph.canonicalization_task(chain_tip, CanonicalizationParams::default());
    let canonical_view = oracle.canonicalize(task, chain_tip);

    // Display unspent outputs
    let unspent: Vec<_> = canonical_view
        .filter_unspent_outpoints(graph.index.outpoints().clone())
        .collect();

    if !unspent.is_empty() {
        println!("\nUnspent");
        for (index, utxo) in unspent {
            // (k, index) | value | outpoint |
            println!("{:?} | {} | {}", index, utxo.txout.value, utxo.outpoint);
        }
    }

    for canon_tx in canonical_view.txs() {
        if !canon_tx.pos.is_confirmed() {
            eprintln!("ERROR: canonical tx should be confirmed {}", canon_tx.txid);
        }
    }

    Ok(())
}
