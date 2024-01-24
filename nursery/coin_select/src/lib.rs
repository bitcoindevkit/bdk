#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[macro_use]
extern crate alloc;
extern crate bdk_chain;

use alloc::vec::Vec;
use bdk_chain::{
    bitcoin,
    collections::{BTreeSet, HashMap},
};
use bitcoin::{absolute, Transaction, TxOut};
use core::fmt::{Debug, Display};

mod coin_selector;
pub use coin_selector::*;

mod bnb;
pub use bnb::*;

/// Txin "base" fields include `outpoint` (32+4) and `nSequence` (4). This does not include
/// `scriptSigLen` or `scriptSig`.
pub const TXIN_BASE_WEIGHT: u32 = (32 + 4 + 4) * 4;

/// Helper to calculate varint size. `v` is the value the varint represents.
// Shamelessly copied from
// https://github.com/rust-bitcoin/rust-miniscript/blob/d5615acda1a7fdc4041a11c1736af139b8c7ebe8/src/util.rs#L8
pub(crate) fn varint_size(v: usize) -> u32 {
    bitcoin::VarInt(v as u64).size() as u32
}
