// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

pub extern crate bitcoin;
extern crate log;
pub extern crate miniscript;
extern crate serde;
#[macro_use]
extern crate serde_json;

#[cfg(any(target_arch = "wasm32", feature = "async-interface"))]
#[macro_use]
extern crate async_trait;
#[macro_use]
extern crate magical_macros;

#[cfg(any(test, feature = "compact_filters"))]
#[macro_use]
extern crate lazy_static;

#[cfg(feature = "electrum")]
pub extern crate electrum_client;

#[cfg(feature = "esplora")]
pub extern crate reqwest;

#[cfg(feature = "key-value-db")]
pub extern crate sled;

#[cfg(feature = "cli-utils")]
pub mod cli;

#[cfg(test)]
#[macro_use]
extern crate testutils;
#[cfg(test)]
#[macro_use]
extern crate testutils_macros;
#[cfg(test)]
#[macro_use]
extern crate serial_test;

#[macro_use]
pub(crate) mod error;
pub mod blockchain;
pub mod database;
pub mod descriptor;
pub(crate) mod psbt;
pub(crate) mod types;
pub mod wallet;

pub use descriptor::HDKeyPaths;
pub use error::Error;
pub use types::*;
pub use wallet::address_validator;
pub use wallet::signer;
pub use wallet::tx_builder::TxBuilder;
pub use wallet::{OfflineWallet, Wallet};
