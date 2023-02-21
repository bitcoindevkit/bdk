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
//
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
//! ## Generate a few addresses
//!
//! ### Example
//! ```
//! use bdk::{Wallet};
//! use bdk::wallet::AddressIndex::New;
//!
//! fn main() -> Result<(), bdk::Error> {
//!     let mut wallet = Wallet::new_no_persist(
//!          "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)",
//!          Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"),
//!          bitcoin::Network::Testnet,
//!     )?;
//!
//!     println!("Address #0: {}", wallet.get_address(New));
//!     println!("Address #1: {}", wallet.get_address(New));
//!     println!("Address #2: {}", wallet.get_address(New));
//!
//!     Ok(())
//! }
//! ```
//! ## Sign a transaction
//!
//! ```no_run
//! use core::str::FromStr;
//!
//! use bitcoin::util::psbt::PartiallySignedTransaction as Psbt;
//!
//! use bdk::{Wallet, SignOptions};
//!
//! fn main() -> Result<(), bdk::Error> {
//!     let wallet = Wallet::new_no_persist(
//!         "wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/0/*)",
//!         Some("wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/1/*)"),
//!         bitcoin::Network::Testnet,
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

#![no_std]
#[cfg(feature = "std")]
#[macro_use]
extern crate std;

#[doc(hidden)]
#[macro_use]
pub extern crate alloc;

pub extern crate bitcoin;
#[cfg(feature = "hardware-signer")]
pub extern crate hwi;
extern crate log;
pub extern crate miniscript;
extern crate serde;
extern crate serde_json;

#[cfg(feature = "keys-bip39")]
extern crate bip39;

#[allow(unused_imports)]
#[macro_use]
pub(crate) mod error;
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
pub use wallet::Wallet;

/// Get the version of BDK at runtime
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION", "unknown")
}

pub use bdk_chain as chain;
pub(crate) use bdk_chain::collections;
