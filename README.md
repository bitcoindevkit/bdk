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

The `bdk` library aims to be the core building block for Bitcoin wallets of any kind.

* It uses [Miniscript](https://github.com/rust-bitcoin/rust-miniscript) to support descriptors with generalized conditions. This exact same library can be used to build
  single-sig wallets, multisigs, timelocked contracts and more.
* It supports multiple blockchain backends and databases, allowing developers to choose exactly what's right for their projects.
* It's built to be cross-platform: the core logic works on desktop, mobile, and even WebAssembly.
* It's very easy to extend: developers can implement customized logic for blockchain backends, databases, signers, coin selection, and more, without having to fork and modify this library.

## Examples

### Sync the balance of a descriptor

```rust,no_run
use bdk::Wallet;
use bdk::database::MemoryDatabase;
use bdk::blockchain::ElectrumBlockchain;
use bdk::SyncOptions;
use bdk::electrum_client::Client;
use bdk::bitcoin::Network;

fn main() -> Result<(), bdk::Error> {
    let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?);
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    wallet.sync(&blockchain, SyncOptions::default())?;

    println!("Descriptor balance: {} SAT", wallet.get_balance()?);

    Ok(())
}
```

### Generate a few addresses

```rust
use bdk::{Wallet, database::MemoryDatabase};
use bdk::wallet::AddressIndex::New;
use bdk::bitcoin::Network;

fn main() -> Result<(), bdk::Error> {
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    println!("Address #0: {}", wallet.get_address(New)?);
    println!("Address #1: {}", wallet.get_address(New)?);
    println!("Address #2: {}", wallet.get_address(New)?);

    Ok(())
}
```

### Create a transaction

```rust,no_run
use bdk::{FeeRate, Wallet, SyncOptions};
use bdk::database::MemoryDatabase;
use bdk::blockchain::ElectrumBlockchain;

use bdk::electrum_client::Client;
use bdk::wallet::AddressIndex::New;

use base64;
use bdk::bitcoin::consensus::serialize;
use bdk::bitcoin::Network;

fn main() -> Result<(), bdk::Error> {
    let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?);
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    wallet.sync(&blockchain, SyncOptions::default())?;

    let send_to = wallet.get_address(New)?;
    let (psbt, details) = {
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(send_to.script_pubkey(), 50_000)
            .enable_rbf()
            .do_not_spend_change()
            .fee_rate(FeeRate::from_sat_per_vb(5.0));
        builder.finish()?
    };

    println!("Transaction details: {:#?}", details);
    println!("Unsigned PSBT: {}", base64::encode(&serialize(&psbt)));

    Ok(())
}
```

### Sign a transaction

```rust,no_run
use bdk::{Wallet, SignOptions, database::MemoryDatabase};

use base64;
use bdk::bitcoin::consensus::deserialize;
use bdk::bitcoin::Network;

fn main() -> Result<(), bdk::Error> {
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/1/*)"),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    let psbt = "...";
    let mut psbt = deserialize(&base64::decode(psbt).unwrap())?;

    let _finalized = wallet.sign(&mut psbt, SignOptions::default())?;

    Ok(())
}
```

## Testing

### Unit testing

```bash
cargo test
```

### Integration testing

Integration testing require testing features, for example:

```bash
cargo test --features test-electrum
```

The other options are `test-esplora`, `test-rpc` or `test-rpc-legacy` which runs against an older version of Bitcoin Core.
Note that `electrs` and `bitcoind` binaries are automatically downloaded (on mac and linux), to specify you already have installed binaries you must use `--no-default-features` and provide `BITCOIND_EXE` and `ELECTRS_EXE` as environment variables.

## Running under WASM

If you want to run this library under WASM you will probably have to add the following lines to you `Cargo.toml`:

```toml
[dependencies]
getrandom = { version = "0.2", features = ["js"] }
```

This enables the `rand` crate to work in environments where JavaScript is available. See [this link](https://docs.rs/getrandom/0.2.8/getrandom/#webassembly-support) to learn more.

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

## Minimum Supported Rust Version (MSRV)

This library should compile with any combination of features with Rust 1.57.0.

To build with the MSRV you will need to pin dependencies as follows:

```shell
# log 0.4.19 has MSRV 1.60.0
cargo update -p log --precise "0.4.18"
# tempfile 3.7.0 has MSRV 1.63.0
cargo update -p tempfile --precise "3.6.0"
# required for sqlite feature, hashlink 0.8.2 has MSRV 1.61.0
cargo update -p hashlink --precise "0.8.1"
# required for compact_filters feature, regex after 1.7.3 has MSRV 1.60.0
cargo update -p regex --precise "1.7.3"
# zip 0.6.3 has MSRV 1.59.0 but still works
cargo update -p zip --precise "0.6.3"
# base64ct 1.6.0 has MSRV 1.60.0
cargo update -p base64ct --precise "1.5.3"
# rustix 0.38.0 has MSRV 1.65.0
cargo update -p rustix --precise "0.37.23"
# tokio 0.30.0 has MSRV 1.63.0
cargo update -p tokio --precise "1.29.1"
# cc 1.0.82 is throwing error with rust 1.57.0, "error[E0599]: no method named `retain_mut`..."
cargo update -p cc --precise "1.0.81"
```
