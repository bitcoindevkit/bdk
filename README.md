<div align="center">
  <h1>BDK</h1>

  <img src="./static/bdk.svg" width="220" />

  <p>
    <strong>A modern, lightweight, descriptor-based wallet library written in Rust!</strong>
  </p>

  <p>
    <!-- <a href="https://crates.io/crates/magical"><img alt="Crate Info" src="https://img.shields.io/crates/v/magical.svg"/></a> -->
    <a href="https://github.com/bitcoindevkit/bdk/actions?query=workflow%3ACI"><img alt="CI Status" src="https://github.com/bitcoindevkit/bdk/workflows/CI/badge.svg"></a>
    <a href="https://codecov.io/gh/bitcoindevkit/bdk"><img src="https://codecov.io/gh/bitcoindevkit/bdk/branch/master/graph/badge.svg"/></a>
    <a href="https://bitcoindevkit.org/docs-rs/bdk"><img alt="API Docs" src="https://img.shields.io/badge/docs.rs-bdk-green"/></a>
    <a href="https://blog.rust-lang.org/2020/07/16/Rust-1.45.0.html"><img alt="Rustc Version 1.45+" src="https://img.shields.io/badge/rustc-1.45%2B-lightgrey.svg"/></a>
    <a href="https://discord.gg/d7NkDKm"><img alt="Chat on Discord" src="https://img.shields.io/discord/753336465005608961?logo=discord"></a>
  </p>

  <h4>
    <a href="https://bitcoindevkit.org">Project Homepage</a>
    <span> | </span>
    <a href="https://bitcoindevkit.org/docs-rs/bdk">Documentation</a>
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
use bdk::blockchain::{noop_progress, ElectrumBlockchain};

use bdk::electrum_client::Client;

fn main() -> Result<(), bdk::Error> {
    let client = Client::new("ssl://electrum.blockstream.info:60002", None)?;
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
        ElectrumBlockchain::from(client)
    )?;

    wallet.sync(noop_progress(), None)?;

    println!("Descriptor balance: {} SAT", wallet.get_balance()?);

    Ok(())
}
```

### Generate a few addresses

```rust
use bdk::{Wallet, OfflineWallet};
use bdk::database::MemoryDatabase;

fn main() -> Result<(), bdk::Error> {
    let wallet: OfflineWallet<_> = Wallet::new_offline(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
    )?;

    println!("Address #0: {}", wallet.get_new_address()?);
    println!("Address #1: {}", wallet.get_new_address()?);
    println!("Address #2: {}", wallet.get_new_address()?);

    Ok(())
}
```

### Create a transaction

```rust,no_run
use bdk::{FeeRate, TxBuilder, Wallet};
use bdk::database::MemoryDatabase;
use bdk::blockchain::{noop_progress, ElectrumBlockchain};

use bdk::electrum_client::Client;

use bitcoin::consensus::serialize;

fn main() -> Result<(), bdk::Error> {
    let client = Client::new("ssl://electrum.blockstream.info:60002", None)?;
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
        ElectrumBlockchain::from(client)
    )?;

    wallet.sync(noop_progress(), None)?;

    let send_to = wallet.get_new_address()?;
    let (psbt, details) = wallet.create_tx(
        TxBuilder::with_recipients(vec![(send_to.script_pubkey(), 50_000)])
            .enable_rbf()
            .do_not_spend_change()
            .fee_rate(FeeRate::from_sat_per_vb(5.0))
    )?;

    println!("Transaction details: {:#?}", details);
    println!("Unsigned PSBT: {}", base64::encode(&serialize(&psbt)));

    Ok(())
}
```

### Sign a transaction

```rust,no_run
use bdk::{Wallet, OfflineWallet};
use bdk::database::MemoryDatabase;

use bitcoin::consensus::deserialize;

fn main() -> Result<(), bdk::Error> {
    let wallet: OfflineWallet<_> = Wallet::new_offline(
        "wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
    )?;

    let psbt = "...";
    let psbt = deserialize(&base64::decode(psbt).unwrap())?;

    let (signed_psbt, finalized) = wallet.sign(psbt, None)?;

    Ok(())
}
```
