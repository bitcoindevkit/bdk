#!/usr/bin/env sh

echo "Starting bitcoin node."
/root/bitcoind -regtest -server -daemon -fallbackfee=0.0002 -rpcuser=$BDK_RPC_USER -rpcpassword=$BDK_RPC_PASS -rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0 -blockfilterindex=1 -peerblockfilters=1

echo "Waiting for bitcoin node."
until /root/bitcoin-cli -regtest -rpcuser=$BDK_RPC_USER -rpcpassword=$BDK_RPC_PASS getblockchaininfo; do
    sleep 1
done
/root/bitcoin-cli -regtest -rpcuser=$BDK_RPC_USER -rpcpassword=$BDK_RPC_PASS createwallet $BDK_RPC_WALLET
echo "Generating 150 bitcoin blocks."
ADDR=$(/root/bitcoin-cli -regtest -rpcuser=$BDK_RPC_USER -rpcpassword=$BDK_RPC_PASS -rpcwallet=$BDK_RPC_WALLET getnewaddress)
/root/bitcoin-cli -regtest -rpcuser=$BDK_RPC_USER -rpcpassword=$BDK_RPC_PASS generatetoaddress 150 $ADDR

echo "Starting electrs node."
nohup /root/electrs --network regtest --jsonrpc-import &
sleep 5
