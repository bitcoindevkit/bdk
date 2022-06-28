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
    <a href="https://codecov.io/gh/bitcoindevkit/bdk"><img src="https://codecov.io/gh/bitcoindevkit/bdk/branch/master/graph/badge.svg"/></a>
    <a href="https://docs.rs/bdk"><img alt="API Docs" src="https://img.shields.io/badge/docs.rs-bdk-green"/></a>
    <a href="https://blog.rust-lang.org/2021/11/01/Rust-1.56.1.html"><img alt="Rustc Version 1.56.1+" src="https://img.shields.io/badge/rustc-1.56.1%2B-lightgrey.svg"/></a>
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

fn main() -> Result<(), bdk::Error> {
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
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

use bitcoin::consensus::serialize;

fn main() -> Result<(), bdk::Error> {
    let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?);
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
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

use bitcoin::consensus::deserialize;

fn main() -> Result<(), bdk::Error> {
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
    )?;

    let psbt = "...";
    let mut psbt = deserialize(&base64::decode(psbt).unwrap())?;

    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;

    Ok(())
}
```

## Testing

### Unit testing

```
cargo test
```

### Integration testing

Integration testing require testing features, for example:

```
cargo test --features test-electrum
```

The other options are `test-esplora`, `test-rpc` or `test-rpc-legacy` which runs against an older version of Bitcoin Core.
Note that `electrs` and `bitcoind` binaries are automatically downloaded (on mac and linux), to specify you already have installed binaries you must use `--no-default-features` and provide `BITCOIND_EXE` and `ELECTRS_EXE` as environment variables.

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
