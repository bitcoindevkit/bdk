# BDK Electrum

BDK Electrum extends [`electrum-client`] to update [`bdk_chain`] structures
from an Electrum server.

## Minimum Supported Rust Version (MSRV)
This crate has a MSRV of 1.75.0.

To build with MSRV you will need to pin dependencies as follows:
```shell
cargo update -p home --precise "0.5.9"
```

[`electrum-client`]: https://docs.rs/electrum-client/
[`bdk_chain`]: https://docs.rs/bdk-chain/
