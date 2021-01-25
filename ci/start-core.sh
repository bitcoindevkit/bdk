#!/usr/bin/env sh

echo "Starting bitcoin node."
/root/bitcoind -regtest -server -daemon -fallbackfee=0.0002 -rpcuser=admin -rpcpassword=passw -rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0 -blockfilterindex=1 -peerblockfilters=1

echo "Waiting for bitcoin node."
until /root/bitcoin-cli -regtest -rpcuser=admin -rpcpassword=passw getblockchaininfo; do
    sleep 1
done
/root/bitcoind -regtest -rpcuser=admin -rpcpassword=passw createwallet bdk-test
echo "Generating 150 bitcoin blocks."
ADDR=$(/root/bitcoin-cli -regtest -rpcuser=admin -rpcpassword=passw getnewaddress)
/root/bitcoin-cli -regtest -rpcuser=admin -rpcpassword=passw generatetoaddress 150 $ADDR

echo "Starting electrs node."
nohup /root/electrs --network regtest --jsonrpc-import &
sleep 5
