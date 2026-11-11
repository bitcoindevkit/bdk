# BDK Bitcoind RPC

This crate is used for emitting blockchain data from the `bitcoind` RPC interface. It does not use
the wallet RPC API, so it can be used with wallet-disabled Bitcoin Core nodes.

[`Emitter`] is the main structure which sources blockchain data from [`bitcoincore_rpc::Client`].
[`FilterIter`] instead scans the chain with BIP158 compact block filters.

## Usage

Give the [`Emitter`] the tip of the chain you already have (for a fresh wallet, just the genesis
block). Then:

- Call [`Emitter::next_block`] until it returns `Ok(None)` (the chain tip is reached). Each
  [`BlockEvent`] carries a [`CheckPoint`] that connects to the chain you started with, so it can be
  applied to a `LocalChain`.
- Call [`Emitter::mempool`] to emit the mempool, along with transactions evicted since the last
  call.

```rust,no_run
use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client},
    Emitter, NO_EXPECTED_MEMPOOL_TXS,
};
use bdk_chain::local_chain::LocalChain;
use bitcoin::{constants::genesis_block, Network};

let (mut chain, _) = LocalChain::from_genesis(genesis_block(Network::Bitcoin).block_hash());

let client = Client::new(
    "127.0.0.1:8332",
    Auth::CookieFile("/home/user/.bitcoin/.cookie".into()),
)?;
let start_height = 0;
let mut emitter = Emitter::new(&client, chain.tip(), start_height, NO_EXPECTED_MEMPOOL_TXS);

while let Some(event) = emitter.next_block()? {
    chain.apply_update(event.checkpoint)?;
    // also apply `event.block` to your indexed tx graph here
}

let mempool = emitter.mempool()?;
// apply `mempool.update` and `mempool.evicted` to your indexed tx graph here
# Ok::<_, Box<dyn std::error::Error>>(())
```

## Trust in the RPC connection

The recommended practice is to connect only to a `bitcoind` node that **you run and control**.
Bitcoin Core's RPC is a privileged, administrative interface rather than a public API. A wallet
inherently trusts its chain data source for the validity of blocks and transactions, for
confirmation status, and for privacy.

If you must reach a node over a network, tunnel the connection (for example over SSH, a VPN, or a
Tor hidden service) and never expose RPC to the public internet. As a defense mechanism, the
emitter verifies that a fetched transaction's computed txid matches the one requested, but this is
not a substitute for connecting to a node you trust.

[`Emitter`]: crate::Emitter
[`Emitter::next_block`]: crate::Emitter::next_block
[`Emitter::mempool`]: crate::Emitter::mempool
[`BlockEvent`]: crate::BlockEvent
[`FilterIter`]: crate::bip158::FilterIter
[`CheckPoint`]: bdk_core::CheckPoint
[`bitcoincore_rpc::Client`]: crate::bitcoincore_rpc::Client
