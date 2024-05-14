# Wallet RPC Example 

```
$ cargo run --bin wallet_rpc -- --help

wallet_rpc 0.1.0
Bitcoind RPC example using `bdk_wallet::Wallet`

USAGE:
    wallet_rpc [OPTIONS] <DESCRIPTOR> <CHANGE_DESCRIPTOR>

ARGS:
    <DESCRIPTOR>           Wallet descriptor [env: DESCRIPTOR=]
    <CHANGE_DESCRIPTOR>    Wallet change descriptor [env: CHANGE_DESCRIPTOR=]

OPTIONS:
    -h, --help
            Print help information

        --network <NETWORK>
            Bitcoin network to connect to [env: BITCOIN_NETWORK=] [default: signet]

        --rpc-cookie <RPC_COOKIE>
            RPC auth cookie file [env: RPC_COOKIE=]

        --rpc-pass <RPC_PASS>
            RPC auth password [env: RPC_PASS=]

        --rpc-user <RPC_USER>
            RPC auth username [env: RPC_USER=]

        --start-height <START_HEIGHT>
            Earliest block height to start sync from [env: START_HEIGHT=] [default: 100000]

        --url <URL>
            RPC URL [env: RPC_URL=] [default: 127.0.0.1:38332]

    -V, --version
            Print version information

```

