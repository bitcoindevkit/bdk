//! This crate is used for emitting blockchain data from the `bitcoind` RPC interface. It does not
//! use the wallet RPC API, so this crate can be used with wallet-disabled Bitcoin Core nodes.
//!
//! [`Emitter`] is the main structure which sources blockchain data from
//! [`bitcoind_client::bitreq::Client`].
//!
//! To only get block updates (exclude mempool transactions), the caller can use
//! [`Emitter::next_block`] until it returns `Ok(None)` (which means the chain tip is reached). A
//! separate method, [`Emitter::mempool`] can be used to emit the whole mempool.
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
