# Example bitcoind RPC sync

### Simple Signet Test with FilterIter

1. Start local signet bitcoind. (~8 GB space required)
   ```
    mkdir -p /tmp/signet/bitcoind
    bitcoind -signet -server -fallbackfee=0.0002 -blockfilterindex -datadir=/tmp/signet/bitcoind -daemon
    tail -f /tmp/signet/bitcoind/signet/debug.log
   ```
   Watch debug.log and wait for bitcoind to finish syncing.

2. Set bitcoind env variables.
   ```
   export RPC_URL=127.0.0.1:38332
   export RPC_COOKIE=/tmp/signet/bitcoind/signet/.cookie
   ```
3. Run `filter_iter` example.
   ```
   cargo run -p bdk_bitcoind_rpc --example filter_iter
   ```