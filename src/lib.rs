// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

// rustdoc will warn if there are missing docs
#![warn(missing_docs)]
// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    docsrs,
    doc(html_logo_url = "https://github.com/bitcoindevkit/bdk/raw/master/static/bdk.png")
)]

//! A modern, lightweight, descriptor-based wallet library written in Rust.
//!
//! # About
//!
//! The BDK library aims to be the core building block for Bitcoin wallets of any kind.
//!
//! * It uses [Miniscript](https://github.com/rust-bitcoin/rust-miniscript) to support descriptors with generalized conditions. This exact same library can be used to build
//!   single-sig wallets, multisigs, timelocked contracts and more.
//! * It supports multiple blockchain backends and databases, allowing developers to choose exactly what's right for their projects.
//! * It is built to be cross-platform: the core logic works on desktop, mobile, and even WebAssembly.
//! * It is very easy to extend: developers can implement customized logic for blockchain backends, databases, signers, coin selection, and more, without having to fork and modify this library.
//!
//! # A Tour of BDK
//!
//! BDK consists of a number of modules that provide a range of functionality
//! essential for implementing descriptor based Bitcoin wallet applications in Rust. In this
//! section, we will take a brief tour of BDK, summarizing the major APIs and
//! their uses.
//!
//! The easiest way to get started is to add bdk to your dependencies with the default features.
//! The default features include a simple key-value database ([`sled`](sled)) to cache
//! blockchain data and an [electrum](https://docs.rs/electrum-client/) blockchain client to
//! interact with the bitcoin P2P network.
//!
//! # Examples
#![cfg_attr(
    feature = "electrum",
    doc = r##"
## Sync the balance of a descriptor

```no_run
use bdk::{Wallet, SyncOptions};
use bdk::database::MemoryDatabase;
use bdk::blockchain::ElectrumBlockchain;
use bdk::electrum_client::Client;

fn main() -> Result<(), bdk::Error> {
    let client = Client::new("ssl://electrum.blockstream.info:60002")?;
    let blockchain = ElectrumBlockchain::from(client);
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
    )?;

    wallet.sync(&blockchain, SyncOptions::default())?;

    println!("Descriptor balance: {} SAT", wallet.get_balance()?);

    Ok(())
}
```
"##
)]
//!
//! ## Generate a few addresses
//!
//! ### Example
//! ```
//! use bdk::{Wallet};
//! use bdk::database::MemoryDatabase;
//! use bdk::wallet::AddressIndex::New;
//!
//! fn main() -> Result<(), bdk::Error> {
//! let wallet = Wallet::new(
//!         "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
//!         Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
//!         bitcoin::Network::Testnet,
//!         MemoryDatabase::default(),
//!     )?;
//!
//!     println!("Address #0: {}", wallet.get_address(New)?);
//!     println!("Address #1: {}", wallet.get_address(New)?);
//!     println!("Address #2: {}", wallet.get_address(New)?);
//!
//!     Ok(())
//! }
//! ```
#![cfg_attr(
    feature = "electrum",
    doc = r##"
## Create a transaction

```no_run
use bdk::{FeeRate, Wallet, SyncOptions};
use bdk::database::MemoryDatabase;
use bdk::blockchain::ElectrumBlockchain;
use bdk::electrum_client::Client;

use bitcoin::consensus::serialize;
use bdk::wallet::AddressIndex::New;

fn main() -> Result<(), bdk::Error> {
    let client = Client::new("ssl://electrum.blockstream.info:60002")?;
    let wallet = Wallet::new(
        "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
        Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
        bitcoin::Network::Testnet,
        MemoryDatabase::default(),
    )?;
    let blockchain = ElectrumBlockchain::from(client);

    wallet.sync(&blockchain, SyncOptions::default())?;

    let send_to = wallet.get_address(New)?;
    let (psbt, details) = {
        let mut builder =  wallet.build_tx();
        builder
            .add_recipient(send_to.script_pubkey(), 50_000)
            .enable_rbf()
            .do_not_spend_change()
            .fee_rate(FeeRate::from_sat_per_vb(5.0));
        builder.finish()?
    };

    println!("Transaction details: {:#?}", details);
    println!("Unsigned PSBT: {}", &psbt);

    Ok(())
}
```
"##
)]
//!
//! ## Sign a transaction
//!
//! ```no_run
//! use std::str::FromStr;
//!
//! use bitcoin::util::psbt::PartiallySignedTransaction as Psbt;
//!
//! use bdk::{Wallet, SignOptions};
//! use bdk::database::MemoryDatabase;
//!
//! fn main() -> Result<(), bdk::Error> {
//!     let wallet = Wallet::new(
//!         "wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/0/*)",
//!         Some("wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/1/*)"),
//!         bitcoin::Network::Testnet,
//!         MemoryDatabase::default(),
//!     )?;
//!
//!     let psbt = "...";
//!     let mut psbt = Psbt::from_str(psbt)?;
//!
//!     let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Feature flags
//!
//! BDK uses a set of [feature flags](https://doc.rust-lang.org/cargo/reference/manifest.html#the-features-section)
//! to reduce the amount of compiled code by allowing projects to only enable the features they need.
//! By default, BDK enables two internal features, `key-value-db` and `electrum`.
//!
//! If you are new to BDK we recommended that you use the default features which will enable
//! basic descriptor wallet functionality. More advanced users can disable the `default` features
//! (`--no-default-features`) and build the BDK library with only the features you need.

//! Below is a list of the available feature flags and the additional functionality they provide.
//!
//! * `all-keys`: all features for working with bitcoin keys
//! * `async-interface`: async functions in bdk traits
//! * `keys-bip39`: [BIP-39](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki) mnemonic codes for generating deterministic keys
//!
//! # Internal features
//!
//! These features do not expose any new API, but influence internal implementation aspects of
//! BDK.
//!
//! * `compact_filters`: [`compact_filters`](crate::blockchain::compact_filters) client protocol for interacting with the bitcoin P2P network
//! * `electrum`: [`electrum`](crate::blockchain::electrum) client protocol for interacting with electrum servers
//! * `esplora`: [`esplora`](crate::blockchain::esplora) client protocol for interacting with blockstream [electrs](https://github.com/Blockstream/electrs) servers
//! * `key-value-db`: key value [`database`](crate::database) based on [`sled`](crate::sled) for caching blockchain data

pub extern crate bitcoin;
extern crate log;
pub extern crate miniscript;
extern crate serde;
#[macro_use]
extern crate serde_json;
#[cfg(feature = "hardware-signer")]
pub extern crate hwi;

#[cfg(all(feature = "reqwest", feature = "ureq"))]
compile_error!("Features reqwest and ureq are mutually exclusive and cannot be enabled together");

#[cfg(all(feature = "async-interface", feature = "electrum"))]
compile_error!(
    "Features async-interface and electrum are mutually exclusive and cannot be enabled together"
);

#[cfg(all(feature = "async-interface", feature = "ureq"))]
compile_error!(
    "Features async-interface and ureq are mutually exclusive and cannot be enabled together"
);

#[cfg(all(feature = "async-interface", feature = "compact_filters"))]
compile_error!(
    "Features async-interface and compact_filters are mutually exclusive and cannot be enabled together"
);

#[cfg(feature = "keys-bip39")]
extern crate bip39;

#[cfg(feature = "async-interface")]
#[macro_use]
extern crate async_trait;
#[macro_use]
extern crate bdk_macros;

#[cfg(feature = "rpc")]
pub extern crate bitcoincore_rpc;

#[cfg(feature = "electrum")]
pub extern crate electrum_client;

#[cfg(feature = "esplora")]
pub extern crate esplora_client;

#[cfg(feature = "key-value-db")]
pub extern crate sled;

#[cfg(feature = "sqlite")]
pub extern crate rusqlite;

// We should consider putting this under a feature flag but we need the macro in doctests so we need
// to wait until https://github.com/rust-lang/rust/issues/67295 is fixed.
//
// Stuff in here is too rough to document atm
#[doc(hidden)]
#[macro_use]
pub mod testutils;

#[cfg(test)]
extern crate assert_matches;

#[allow(unused_imports)]
#[macro_use]
pub(crate) mod error;
pub mod blockchain;
pub mod database;
pub mod descriptor;
#[cfg(feature = "test-md-docs")]
mod doctest;
pub mod keys;
pub mod psbt;
pub(crate) mod types;
pub mod wallet;

pub use descriptor::template;
pub use descriptor::HdKeyPaths;
pub use error::Error;
pub use types::*;
pub use wallet::signer;
pub use wallet::signer::SignOptions;
pub use wallet::tx_builder::TxBuilder;
pub use wallet::SyncOptions;
pub use wallet::Wallet;

/// Get the version of BDK at runtime
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION", "unknown")
}
