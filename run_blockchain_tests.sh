#!/bin/sh

usage() {
    cat <<'EOF'
Script for running the bdk blockchain tests for a specific blockchain by starting up the backend in docker.

Usage: ./run_blockchain_tests.sh [esplora|electrum|rpc] [test name].

EOF
}

eprintln(){
    echo "$@" >&2
}

cleanup() {
    if test "$id"; then
        eprintln "cleaning up $blockchain docker container $id";
        docker rm -fv "$id" > /dev/null;
        rm /tmp/regtest-"$id".cookie;
    fi
    trap - EXIT INT
}

# Makes sure we clean up the container at the end or if ^C
trap 'rc=$?; cleanup; exit $rc' EXIT INT

blockchain="$1"
test_name="$2"

case "$blockchain" in
    electrum)
        eprintln "starting electrs docker container"
        id="$(docker run --detach -p 127.0.0.1:18443-18444:18443-18444/tcp -p 127.0.0.1:60401:60401/tcp bitcoindevkit/electrs:0.4.0)"
        ;;
    esplora)
        eprintln "starting esplora docker container"
        id="$(docker run --detach -p 127.0.0.1:18443-18444:18443-18444/tcp -p 127.0.0.1:60401:60401/tcp -p 127.0.0.1:3002:3002/tcp bitcoindevkit/esplora:0.4.0)"
        export BDK_ESPLORA_URL=http://127.0.0.1:3002
        ;;
    rpc)
        eprintln "starting bitcoind docker container (via electrs container)"
        id="$(docker run --detach -p 127.0.0.1:18443-18444:18443-18444/tcp -p 127.0.0.1:60401:60401/tcp bitcoindevkit/electrs:0.4.0)"      
        ;;
    *)
        usage;
        exit 1;
        ;;
    esac

# taken from https://github.com/bitcoindevkit/bitcoin-regtest-box
export BDK_RPC_AUTH=COOKIEFILE
export BDK_RPC_COOKIEFILE=/tmp/regtest-"$id".cookie
export BDK_RPC_URL=127.0.0.1:18443
export BDK_RPC_WALLET=bdk-test
export BDK_ELECTRUM_URL=tcp://127.0.0.1:60401

cli(){
    docker exec -it "$id" /root/bitcoin-cli -regtest -datadir=/root/.bitcoin $@
}

#eprintln "running getwalletinfo until bitcoind seems to be alive"
while ! cli getwalletinfo >/dev/null; do sleep 1; done

# sleep again for good measure!
sleep 1;

# copy bitcoind cookie file to /tmp
docker cp "$id":/root/.bitcoin/regtest/.cookie /tmp/regtest-"$id".cookie

cargo test --features "test-blockchains,test-$blockchain" --no-default-features "$blockchain::bdk_blockchain_tests::$test_name"
