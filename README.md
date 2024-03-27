# The Bitcoin Dev Kit

<div align="center">
  <h1>BDK</h1>

  <img src="./static/bdk.png" width="220" />

  <p>
    <strong>A modern, lightweight, descriptor-based wallet library written in Rust!</strong>
  </p>

  <p>
    <a href="https://crates.io/crates/bdk"><img alt="Crate Info" src="https://img.shields.io/crates/v/bdk.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/blob/master/LICENSE"><img alt="MIT or Apache-2.0 Licensed" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/actions?query=workflow%3ACI"><img alt="CI Status" src="https://github.com/bitcoindevkit/bdk/workflows/CI/badge.svg"></a>
    <a href="https://coveralls.io/github/bitcoindevkit/bdk?branch=master"><img src="https://coveralls.io/repos/github/bitcoindevkit/bdk/badge.svg?branch=master"/></a>
    <a href="https://docs.rs/bdk"><img alt="API Docs" src="https://img.shields.io/badge/docs.rs-bdk-green"/></a>
    <a href="https://blog.rust-lang.org/2022/08/11/Rust-1.63.0.html"><img alt="Rustc Version 1.63.0+" src="https://img.shields.io/badge/rustc-1.63.0%2B-lightgrey.svg"/></a>
    <a href="https://discord.gg/d7NkDKm"><img alt="Chat on Discord" src="https://img.shields.io/discord/753336465005608961?logo=discord"></a>
  </p>

  <h4>
    <a href="https://bitcoindevkit.org">Project Homepage</a>
    <span> | </span>
    <a href="https://docs.rs/bdk">Documentation</a>
  </h4>
</div>

## About

The `bdk` libraries aims to provide well engineered and reviewed components for Bitcoin based applications.
It is built upon the excellent [`rust-bitcoin`] and [`rust-miniscript`] crates.

> ⚠ The Bitcoin Dev Kit developers are in the process of releasing a `v1.0` which is a fundamental re-write of how the library works.
> See for some background on this project: https://bitcoindevkit.org/blog/road-to-bdk-1/ (ignore the timeline 😁)
> For a release timeline see the [`BDK 1.0 project page`].

## Architecture

The project is split up into several crates in the `/crates` directory:

- [`bdk`](./crates/bdk): Contains the central high level `Wallet` type that is built from the low-level mechanisms provided by the other components
- [`chain`](./crates/chain): Tools for storing and indexing chain data
- [`file_store`](./crates/file_store): A (experimental) persistence backend for storing chain data in a single file.
- [`esplora`](./crates/esplora): Extends the [`esplora-client`] crate with methods to fetch chain data from an esplora HTTP server in the form that [`bdk_chain`] and `Wallet` can consume.
- [`electrum`](./crates/electrum): Extends the [`electrum-client`] crate with methods to fetch chain data from an electrum server in the form that [`bdk_chain`] and `Wallet` can consume.
- [`bitcond_rpc`](./crates/bitcond_rpc) Emitting blockchain data from the `bitcoind` RPC interface.

Fully working examples of how to use these components are in `/example-crates`:
- [`example_cli`](./example-crates/example_cli): Library used by the `example_*` crates. Provides utilities for syncing, showing the balance, generating addresses and creating transactions without using the bdk `Wallet`.
- [`example_electrum`](./example-crates/example_electrum): A command line Bitcoin wallet application built on top of `example_cli` and the `electrum` crate. It shows the power of the bdk tools (`chain` + `file_store` + `electrum`), without depending on the main `bdk` library.
- [`example_esplora`](./example-crates/example_esplora): A command line Bitcoin wallet application built on top of `example_cli` and the `esplora` crate. It shows the power of the bdk tools (`chain` + `file_store` + `esplora`), without depending on the main `bdk` library.
- [`example_bitcoind_rpc_polling`](./example-crates/example_bitcoind_rpc_polling): A command line Bitcoin wallet application built on top of `example_cli` and the `bitcoind_rpc` crate. It shows the power of the bdk tools (`chain` + `file_store` + `bitcoind_rpc`), without depending on the main `bdk` library.
- [`wallet_esplora_blocking`](./example-crates/wallet_esplora_blocking): Uses the `Wallet` to sync and spend using the Esplora blocking interface.
- [`wallet_esplora_async`](./example-crates/wallet_esplora_async): Uses the `Wallet` to sync and spend using the Esplora asynchronous interface.
- [`wallet_electrum`](./example-crates/wallet_electrum): Uses the `Wallet` to sync and spend using Electrum.

[`BDK 1.0 project page`]: https://github.com/orgs/bitcoindevkit/projects/14
[`rust-miniscript`]: https://github.com/rust-bitcoin/rust-miniscript
[`rust-bitcoin`]: https://github.com/rust-bitcoin/rust-bitcoin
[`esplora-client`]: https://docs.rs/esplora-client/
[`electrum-client`]: https://docs.rs/electrum-client/
[`bdk_chain`]: https://docs.rs/bdk-chain/

## Dependencies

- `bitcoind_rpc` depends on `bitcoind` being installed and available in `PATH` or in the `BITCOIND_EXEC` ENV variable.
- `esplora` depends on [Blockstream's version of `electrs`](https://github.com/Blockstream/electrs)
   being installed and available in `PATH` or in the `ELECTRS_EXEC` ENV variable.

## Minimum Supported Rust Version (MSRV)

This library should compile with any combination of features with Rust 1.63.0.

To build with the MSRV you will need to pin dependencies as follows:

```shell
# home 0.5.9 has MSRV 1.70.0
cargo update -p home --precise "0.5.5"
```

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
