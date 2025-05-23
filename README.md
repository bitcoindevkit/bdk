# The Bitcoin Dev Kit

<div align="center">

  <img src="./static/bdk.png" width="220" />

  <p>
    <strong>A suite of libraries for building modern, lightweight, descriptor-based wallets written in Rust!</strong>
  </p>

  <p>
    <a href="https://github.com/bitcoindevkit/bdk/blob/master/LICENSE"><img alt="MIT or Apache-2.0 Licensed" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/actions?query=workflow%3ACI"><img alt="CI Status" src="https://github.com/bitcoindevkit/bdk/workflows/CI/badge.svg"></a>
    <a href="https://coveralls.io/github/bitcoindevkit/bdk?branch=master"><img src="https://coveralls.io/repos/github/bitcoindevkit/bdk/badge.svg?branch=master"/></a>
    <a href="https://blog.rust-lang.org/2022/08/11/Rust-1.63.0.html"><img alt="Rustc Version 1.63.0+" src="https://img.shields.io/badge/rustc-1.63.0%2B-lightgrey.svg"/></a>
    <a href="https://discord.gg/d7NkDKm"><img alt="Chat on Discord" src="https://img.shields.io/discord/753336465005608961?logo=discord"></a>
  </p>

  <h4>
    <a href="https://bitcoindevkit.org">Project Homepage</a>
  </h4>
</div>

## About

The `bdk` libraries aim to provide well engineered and reviewed components for Bitcoin wallets and other applications.
They are built upon the excellent [`rust-bitcoin`] and [`rust-miniscript`] crates.

**See also:** [Fee estimation examples and documentation](docs/fee_estimation.md)

## Architecture

The workspace in this repository contains several crates in the `/crates` directory:

| Sub-Directory | Description | Badges |
|---------------|-------------|--------|
| [`chain`](./crates/chain) | Tools for storing and indexing chain data. | ![Chain Crate Info](https://img.shields.io/crates/v/bdk_chain.svg) ![Chain API Docs](https://img.shields.io/badge/docs.rs-bdk_chain-green) |
| [`core`](./crates/core) | A collection of core structures used by the [`bdk_chain`], [`bdk_wallet`], and BDK's chain data source crates. | ![Core Crate Info](https://img.shields.io/crates/v/bdk_core.svg) ![Core API Docs](https://img.shields.io/badge/docs.rs-bdk_core-green) |
| [`esplora`](./crates/esplora) | Extends the [`esplora-client`] crate with methods to fetch chain data from an esplora HTTP server in the form that [`bdk_chain`] and `Wallet` can consume. | ![Esplora Crate Info](https://img.shields.io/crates/v/bdk_esplora.svg) ![Esplora API Docs](https://img.shields.io/badge/docs.rs-bdk_esplora-green) |
| [`electrum`](./crates/electrum) | Extends the [`electrum-client`] crate with methods to fetch chain data from an electrum server in the form that [`bdk_chain`] and `Wallet` can consume. | ![Electrum Crate Info](https://img.shields.io/crates/v/bdk_electrum.svg) ![Electrum API Docs](https://img.shields.io/badge/docs.rs-bdk_electrum-green) |
| [`bitcoind_rpc`](./crates/bitcoind_rpc) | Extends [`bitcoincore-rpc`] for emitting blockchain data from the `bitcoind` RPC interface in the form that [`bdk_chain`] and `Wallet` can consume. | ![BitcoinD RPC Crate Info](https://img.shields.io/crates/v/bdk_bitcoind_rpc.svg) ![BitcoinD RPC API Docs](https://img.shields.io/badge/docs.rs-bdk_bitcoind_rpc-green) |
| [`file_store`](./crates/file_store) | Persistence backend for storing chain data in a single file. Intended for testing and development purposes, not for production. | ![File Store Crate Info](https://img.shields.io/crates/v/bdk_file_store.svg) ![File Store API Docs](https://img.shields.io/badge/docs.rs-bdk_file_store-green) |

The [`bdk_wallet`] repository and crate contains a higher level `Wallet` type that depends on the above lower-level mechanism crates. 

Fully working examples of how to use these components are in `/examples`:

- [`example_cli`](examples/example_cli): Library used by the `example_*` crates. Provides utilities for syncing, showing the balance, generating addresses and creating transactions without using the bdk_wallet `Wallet`.
- [`example_electrum`](examples/example_electrum): A command line Bitcoin wallet application built on top of `example_cli` and the `electrum` crate. It shows the power of the bdk tools (`chain` + `file_store` + `electrum`), without depending on the main `bdk_wallet` library.
- [`example_esplora`](examples/example_esplora): A command line Bitcoin wallet application built on top of `example_cli` and the `esplora` crate. It shows the power of the bdk tools (`chain` + `file_store` + `esplora`), without depending on the main `bdk_wallet` library.
- [`example_bitcoind_rpc_polling`](examples/example_bitcoind_rpc_polling): A command line Bitcoin wallet application built on top of `example_cli` and the `bitcoind_rpc` crate. It shows the power of the bdk tools (`chain` + `file_store` + `bitcoind_rpc`), without depending on the main `bdk_wallet` library.

[`rust-miniscript`]: https://github.com/rust-bitcoin/rust-miniscript
[`rust-bitcoin`]: https://github.com/rust-bitcoin/rust-bitcoin
[`esplora-client`]: https://docs.rs/esplora-client/
[`electrum-client`]: https://docs.rs/electrum-client/
[`bitcoincore-rpc`]: https://docs.rs/bitcoincore-rpc/
[`bdk_chain`]: https://docs.rs/bdk-chain/
[`bdk_wallet`]: https://github.com/bitcoindevkit/bdk_wallet

## Minimum Supported Rust Version (MSRV)

The following BDK crates maintains a MSRV of 1.63.0. To build these crates with the MSRV of 1.63.0 you will need to pin dependencies by running the [`pin-msrv.sh`](./ci/pin-msrv.sh) script.

- `bdk_core`
- `bdk_chain`
- `bdk_bitcoind_rpc`
- `bdk_esplora`
- `bdk_file_store`

The MSRV of the `bdk_electrum` crate is 1.75.0.

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
