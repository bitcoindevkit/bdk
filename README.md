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
    <a href="https://blog.rust-lang.org/2021/12/02/Rust-1.57.0.html"><img alt="Rustc Version 1.57.0+" src="https://img.shields.io/badge/rustc-1.57.0%2B-lightgrey.svg"/></a>
    <a href="https://discord.gg/d7NkDKm"><img alt="Chat on Discord" src="https://img.shields.io/discord/753336465005608961?logo=discord"></a>
  </p>

  <h4>
    <a href="https://bitcoindevkit.org">Project Homepage</a>
    <span> | </span>
    <a href="https://docs.rs/bdk">Documentation</a>
  </h4>
</div>

## About

The `bdk` library aims to provide well-engineered and reviewed components for Bitcoin-based applications.
It is built upon excellent [`rust-bitcoin`] and [`rust-miniscript`] crates.

> ⚠ The Bitcoin Dev Kit developers are in the process of releasing a `v1.0` which is a fundamental re-write of how the library works.
> See for some background on this project: https://bitcoindevkit.org/blog/road-to-bdk-1/ (ignore the timeline 😁)
> For a release timeline see the [`bdk_core_staging`] repo where a lot of the component work is being done. The plan is that everything in the `bdk_core_staging` repo will be moved into the `crates` directory here.

## Architecture

The project is split up into several crates in the `/crates` directory:

- [`bdk`](./crates/bdk): Contains the central high-level `Wallet` type that is built from the low-level mechanisms provided by the other components
- [`chain`](./crates/chain): Tools for storing and indexing chain data
- [`file_store`](./crates/file_store): A (experimental) persistence backend for storing chain data in a single file.
- [`esplora`](./crates/esplora): Extends the [`esplora-client`] crate with methods to fetch chain data from an esplora HTTP server in the form that [`bdk_chain`] and `Wallet` can consume.
- [`electrum`](./crates/electrum): Extends the [`electrum-client`] crate with methods to fetch chain data from an electrum server in the form that [`bdk_chain`] and `Wallet` can consume.

Fully working examples of how to use these components are in `/example-crates`

[`bdk_core_staging`]: https://github.com/LLFourn/bdk_core_staging
[`rust-miniscript`]: https://github.com/rust-bitcoin/rust-miniscript
[`rust-bitcoin`]: https://github.com/rust-bitcoin/rust-bitcoin
[`esplora-client`]: https://docs.rs/esplora-client/0.3.0/esplora_client/
[`electrum-client`]: https://docs.rs/electrum-client/0.13.0/electrum_client/
