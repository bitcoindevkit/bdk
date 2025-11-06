# Example RPC CLI

### Simple Regtest Test

1. Start local regtest bitcoind.
   ```
    mkdir -p /tmp/regtest/bitcoind
    bitcoind -regtest -server -fallbackfee=0.0002 -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> -datadir=/tmp/regtest/bitcoind -daemon
   ```
2. Create a test bitcoind wallet and set bitcoind env.
   ```
   bitcoin-cli -datadir=/tmp/regtest/bitcoind -regtest -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> -named createwallet wallet_name="test"
   export RPC_URL=127.0.0.1:18443
   export RPC_USER=<your-rpc-username>
   export RPC_PASS=<your-rpc-password>
   ```
3. Get test bitcoind wallet info.
   ```
   bitcoin-cli -rpcwallet="test" -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> -datadir=/tmp/regtest/bitcoind -regtest getwalletinfo
   ```
4. Get new test bitcoind wallet address.
   ```
   BITCOIND_ADDRESS=$(bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> getnewaddress)
   echo $BITCOIND_ADDRESS
   ```
5. Generate 101 blocks with reward to test bitcoind wallet address.
   ```
   bitcoin-cli -datadir=/tmp/regtest/bitcoind -regtest -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> generatetoaddress 101 $BITCOIND_ADDRESS
   ```
6. Verify test bitcoind wallet balance.
   ```
   bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> getbalances
   ```
7. Set descriptor env and get address from RPC CLI wallet.
   ```
   export DESCRIPTOR="wpkh(tprv8ZgxMBicQKsPfK9BTf82oQkHhawtZv19CorqQKPFeaHDMA4dXYX6eWsJGNJ7VTQXWmoHdrfjCYuDijcRmNFwSKcVhswzqs4fugE8turndGc/1/*)"
   cargo run -- init --network regtest
   cargo run -- address next
   ```
8. Send 0.05 test bitcoin to RPC CLI wallet.
   ```
   bitcoin-cli -rpcwallet="test" -datadir=/tmp/regtest/bitcoind -regtest -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> sendtoaddress <address> 0.05
   ```
9. Sync blockchain with RPC CLI wallet.
   ```
   cargo run -- sync
   ```
10. Get RPC CLI wallet unconfirmed balances.
   ```
   cargo run -- balance
   ```
11. Generate 1 block with reward to test bitcoind wallet address.
   ```
   bitcoin-cli -datadir=/tmp/regtest/bitcoind -rpcuser=<your-rpc-username> -rpcpassword=<your-rpc-password> -regtest generatetoaddress 1 $BITCOIND_ADDRESS
   ```
12. Sync the blockchain with RPC CLI wallet.
   ```
   cargo run -- sync
   ```
13. Get RPC CLI wallet confirmed balances.
   ```
   cargo run -- balance
   ```
14. Get RPC CLI wallet transactions.
   ```
   cargo run -- txout list
   ```
