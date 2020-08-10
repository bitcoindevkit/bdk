#!/usr/bin/env sh

set -e

BITCOIN_VERSION=0.20.1

# This should be cached by Travis
cargo install --git https://github.com/romanz/electrs --bin electrs

curl -O -L https://bitcoincore.org/bin/bitcoin-core-$BITCOIN_VERSION/bitcoin-$BITCOIN_VERSION-x86_64-linux-gnu.tar.gz
tar xf bitcoin-$BITCOIN_VERSION-x86_64-linux-gnu.tar.gz

export PATH=$PATH:./bitcoin-$BITCOIN_VERSION/bin

bitcoind -regtest=1 -daemon=1 -fallbackfee=0.0002
until bitcoin-cli -regtest getblockchaininfo; do
    sleep 1
done

ADDR=$(bitcoin-cli -regtest getnewaddress)
bitcoin-cli -regtest generatetoaddress 150 $ADDR

nohup electrs --network regtest --jsonrpc-import --cookie-file /home/travis/.bitcoin/regtest/.cookie &
sleep 5
