# ChainQuery Examples

This directory contains examples demonstrating the use of BDK's `ChainQuery` trait for transaction canonicalization without requiring a full local chain store.

## Examples

### bitcoind_rpc_oracle
Uses Bitcoin Core RPC with the `ChainOracle` trait implementation to perform on-demand block verification during canonicalization.

#### Setup for Signet

1. Start local signet bitcoind (~8 GB space required):
   ```bash
   mkdir -p /tmp/signet/bitcoind
   bitcoind -signet -server -fallbackfee=0.0002 -blockfilterindex -datadir=/tmp/signet/bitcoind -daemon
   tail -f /tmp/signet/bitcoind/signet/debug.log
   ```
   Watch debug.log and wait for bitcoind to finish syncing.

2. Set bitcoind environment variables:
   ```bash
   export RPC_URL=127.0.0.1:38332
   export RPC_COOKIE=/tmp/signet/bitcoind/signet/.cookie
   ```

3. Run the example:
   ```bash
   cargo run --bin bitcoind_rpc_oracle
   ```

### kyoto_oracle
Uses Kyoto (BIP157/158 compact block filters) with async on-demand block fetching for canonicalization. Connects to Signet network peers.

To run:
```bash
cargo run --bin kyoto_oracle
```

## Key Concepts

Both examples demonstrate:
- Using `CanonicalizationTask` with the `ChainQuery` trait
- On-demand chain data fetching instead of storing all headers locally
- Processing transaction graphs without a full `LocalChain`

The main difference is the backend:
- `bitcoind_rpc_oracle`: Synchronous RPC calls to Bitcoin Core
- `kyoto_oracle`: Async P2P network communication using compact block filters