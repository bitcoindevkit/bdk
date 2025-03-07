# Wallet RPC Example 

```
$ cargo run --bin example_wallet_rpc -- --help

Bitcoind RPC example using `bdk_wallet::Wallet`

Usage: example_wallet_rpc [OPTIONS] <DESCRIPTOR> [CHANGE_DESCRIPTOR]

Arguments:
  <DESCRIPTOR>         Wallet descriptor [env: DESCRIPTOR=]
  [CHANGE_DESCRIPTOR]  Wallet change descriptor [env: CHANGE_DESCRIPTOR=]

Options:
      --start-height <START_HEIGHT>  Earliest block height to start sync from [env: START_HEIGHT=] [default: 0]

      --network <NETWORK>            Bitcoin network to connect to [env: BITCOIN_NETWORK=] [default: regtest]

      --db-path <DB_PATH>            Where to store wallet data [env: BDK_DB_PATH=] [default: .bdk_wallet_rpc_example.db]

      --url <URL>                    RPC URL [env: RPC_URL=] [default: 127.0.0.1:18443]

      --rpc-cookie <RPC_COOKIE>      RPC auth cookie file [env: RPC_COOKIE=]

      --rpc-user <RPC_USER>          RPC auth username [env: RPC_USER=]

      --rpc-pass <RPC_PASS>          RPC auth password [env: RPC_PASS=]

  -h, --help                         Print help
  
  -V, --version                      Print version

```

