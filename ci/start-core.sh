#!/usr/bin/env sh

set -e

BITCOIN_VERSION=0.20.1

wget -O - https://apt.llvm.org/llvm-snapshot.gpg.key | sudo apt-key add - || exit 1
sudo apt-add-repository "deb http://apt.llvm.org/xenial/ llvm-toolchain-xenial-10 main" || exit 1
sudo apt-get update || exit 1
sudo apt-get install -y libllvm10 clang-10 libclang-common-10-dev || exit 1

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

nohup electrs --network regtest --jsonrpc-import --cookie-file $HOME/.bitcoin/regtest/.cookie &
sleep 5
