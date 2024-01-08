#![doc = include_str!("../README.md")]
// only enables the `doc_cfg` feature when the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    docsrs,
    doc(html_logo_url = "https://github.com/bitcoindevkit/bdk/raw/master/static/bdk.png")
)]
#![no_std]
#![warn(missing_docs)]

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

#[doc(hidden)]
#[macro_use]
pub extern crate alloc;

pub extern crate bitcoin;
pub extern crate miniscript;
extern crate serde;
extern crate serde_json;

#[cfg(feature = "keys-bip39")]
extern crate bip39;

pub mod descriptor;
pub mod keys;
pub mod psbt;
pub(crate) mod types;
pub mod wallet;

pub use descriptor::template;
pub use descriptor::HdKeyPaths;
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
