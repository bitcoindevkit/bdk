#!/usr/bin/env sh

echo "Starting bitcoin node."
mkdir $GITHUB_WORKSPACE/.bitcoin
/root/bitcoind -regtest -server -daemon -datadir=$GITHUB_WORKSPACE/.bitcoin -fallbackfee=0.0002 -rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0 -blockfilterindex=1 -peerblockfilters=1

echo "Waiting for bitcoin node."
until /root/bitcoin-cli -regtest -datadir=$GITHUB_WORKSPACE/.bitcoin getblockchaininfo; do
    sleep 1
done
/root/bitcoin-cli -regtest -datadir=$GITHUB_WORKSPACE/.bitcoin createwallet $BDK_RPC_WALLET
echo "Generating 150 bitcoin blocks."
ADDR=$(/root/bitcoin-cli -regtest -datadir=$GITHUB_WORKSPACE/.bitcoin -rpcwallet=$BDK_RPC_WALLET getnewaddress)
/root/bitcoin-cli -regtest -datadir=$GITHUB_WORKSPACE/.bitcoin generatetoaddress 150 $ADDR
