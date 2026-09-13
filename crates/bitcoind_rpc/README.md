# BDK Bitcoind RPC

This crate emits blockchain data from the `bitcoind` RPC interface. It does not use the wallet
RPC API, so it works against wallet-disabled Bitcoin Core nodes.

[`Emitter`] is the main entry point. It sources blocks and mempool transactions from a
[`bitcoind_client::bitreq::Client`] and produces updates that connect to a local chain you already
hold, handling reorgs along the way.

## Usage

Give the emitter a checkpoint describing the chain you already know about (for a fresh wallet this
is just the genesis block). Then:

- Call [`Emitter::next_block`] in a loop until it returns `Ok(None)` to walk the chain forward,
  block by block, up to the node's tip. Each [`BlockEvent`] carries a [`CheckPoint`] that connects
  to your existing chain, so it can be applied directly to a `LocalChain`.
- Call [`Emitter::mempool`] to emit the current mempool: the full set of unconfirmed transactions
  plus any that have been evicted since the last call.

If your wallet has a known creation height ("birthday"), use [`Emitter::start_height`] to skip
directly to it instead of scanning from the checkpoint height.

```rust,no_run
use bdk_bitcoind_rpc::{Emitter, EmitterError};
use bdk_chain::local_chain::LocalChain;
use bitcoin::{constants::genesis_block, BlockHash, Network, Transaction};
use bitcoind_client::bitreq::{Auth, Client};

// The chain we already know about. A fresh wallet starts at the network's genesis block.
let genesis_hash: BlockHash = genesis_block(Network::Bitcoin).block_hash();
let (local_chain, _) = LocalChain::from_genesis(genesis_hash);

let rpc_client = Client::with_auth(
    "127.0.0.1:8332",
    Auth::CookieFile("/home/user/.bitcoin/.cookie".into()),
)?;

let mut emitter = Emitter::new(
    &rpc_client,
    local_chain.tip(),
    core::iter::empty::<Transaction>(),
);

// Walk the chain forward until the node's tip is reached.
while let Some(event) = emitter.next_block()? {
    println!("block {}: {}", event.block_height(), event.block_hash());
}

// Emit the current mempool state.
let mempool = emitter.mempool()?;
println!("{} mempool txs, {} evicted", mempool.update.len(), mempool.evicted.len());
# <Result<_, EmitterError>>::Ok(())
```

[`Emitter`]: crate::Emitter
[`Emitter::next_block`]: crate::Emitter::next_block
[`Emitter::mempool`]: crate::Emitter::mempool
[`Emitter::start_height`]: crate::Emitter::start_height
[`BlockEvent`]: crate::BlockEvent
[`CheckPoint`]: bdk_core::CheckPoint
[`bitcoind_client::bitreq::Client`]: bitcoind_client::bitreq::Client
