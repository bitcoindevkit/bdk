#![doc = include_str!("../README.md")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

pub extern crate bitcoind_client;

mod emitter;
pub use emitter::*;

pub mod bip158;

pub(crate) use bitcoind_client::corepc_types;
