#![no_std]
// #![warn(missing_docs)]
#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]

#[allow(unused_imports)]
#[macro_use]
extern crate alloc;

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

mod coin_selector;
pub mod float;
pub use coin_selector::*;

mod bnb;
pub use bnb::*;

pub mod metrics;

mod feerate;
pub use feerate::*;
pub mod change_policy;

/// Txin "base" fields include `outpoint` (32+4) and `nSequence` (4) and 1 byte for the scriptSig
/// length.
pub const TXIN_BASE_WEIGHT: u32 = (32 + 4 + 4 + 1) * 4;

/// The weight of a TXOUT without the `scriptPubkey` (and script pubkey length field).
/// Just the weight of the value field.
pub const TXOUT_BASE_WEIGHT: u32 = 4 * core::mem::size_of::<u64>() as u32; // just the value

pub const TR_KEYSPEND_SATISFACTION_WEIGHT: u32 = 66;

/// The weight of a taproot script pubkey
pub const TR_SPK_WEIGHT: u32 = (1 + 1 + 32) * 4; // version + push + key

/// Helper to calculate varint size. `v` is the value the varint represents.
fn varint_size(v: usize) -> u32 {
    if v <= 0xfc {
        return 1;
    }
    if v <= 0xffff {
        return 3;
    }
    if v <= 0xffff_ffff {
        return 5;
    }
    9
}

#[allow(unused)]
fn txout_weight_from_spk_len(spk_len: usize) -> u32 {
    (TXOUT_BASE_WEIGHT + varint_size(spk_len) + (spk_len as u32)) * 4
}
