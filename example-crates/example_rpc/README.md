# Example RPC CLI

### Simple Regtest Test

1. Start local regtest bitcoind.
   ```
    mkdir -p /tmp/regtest/bitcoind
    bitcoind -datadir=/tmp/regtest/bitcoind -regtest -server -fallbackfee=0.0002 -rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0 -blockfilterindex=1 -peerblockfilters=1 -daemon
   ```
2. Create a test bitcoind wallet and set bitcoind env.
   ```
   bitcoin-cli -datadir=/tmp/regtest/bitcoind -regtest -named createwallet wallet_name="test"
   export RPC_URL=127.0.0.1:18443
   export RPC_COOKIE=/tmp/regtest/bitcoind/regtest/.cookie
   ```
3. Get test bitcoind wallet info.
   ```
   bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest getwalletinfo
   ```
4. Get new test bitcoind wallet address.
   ```
   BITCOIND_ADDRESS=$(bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest getnewaddress)
   echo $BITCOIND_ADDRESS
   ```
5. Generate 101 blocks with reward to test bitcoind wallet address.
   ```
   bitcoin-cli -datadir=/tmp/regtest/bitcoind -regtest generatetoaddress 101 $BITCOIND_ADDRESS
   ```
6. Verify test bitcoind wallet balance.
   ```
   bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest getbalances
   ```
7. Set descriptor env and get address from RPC CLI wallet.
   ```
   export DESCRIPTOR="wpkh(tprv8ZgxMBicQKsPfK9BTf82oQkHhawtZv19CorqQKPFeaHDMA4dXYX6eWsJGNJ7VTQXWmoHdrfjCYuDijcRmNFwSKcVhswzqs4fugE8turndGc/1/*)"
   cargo run -- --network regtest address next
   ```
8. Send 5 test bitcoin to RPC CLI wallet.
   ```
   bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest send '[{"<address>":5}]'
   ```
9. Scan blockchain with RPC CLI wallet.
   ```
   cargo run -- --network regtest scan
   <CNTRL-C to stop scanning>
   ```
10. Get RPC CLI wallet unconfirmed balances.
   ```
   cargo run -- --network regtest balance
   ```
11. Generate 1 block with reward to test bitcoind wallet address.
   ```
   bitcoin-cli -datadir=/tmp/regtest/bitcoind -regtest generatetoaddress 10 $BITCOIND_ADDRESS
   ```
12. Scan blockchain with RPC CLI wallet.
   ```
   cargo run -- --network regtest scan
   <CNTRL-C to stop scanning>
   ```
13. Get RPC CLI wallet confirmed balances.
   ```
   cargo run -- --network regtest balance
   ```
14. Get RPC CLI wallet transactions.
   ```
   cargo run -- --network regtest txout list
   ```