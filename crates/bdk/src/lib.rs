#![doc = include_str!("../README.md")]
#![no_std]
#![warn(missing_docs)]

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
