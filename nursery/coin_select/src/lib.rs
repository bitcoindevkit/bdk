#[no_std]
#[allow(unused)]
#[macro_use]
extern crate alloc;

mod coin_selector;
pub mod ord_float;
pub use coin_selector::*;

pub mod bnb;
pub mod metrics;

mod feerate;
pub use feerate::*;
pub mod change_policy;

/// Txin "base" fields include `outpoint` (32+4) and `nSequence` (4). This does not include
/// `scriptSigLen`, `scriptSig` or witness stack length
pub const TXIN_BASE_WEIGHT: u32 = (32 + 4 + 4) * 4;

/// The weight of a TXOUT without the `scriptPubkey` (and script pubkey length field).
/// Just the weight of the value field.
pub const TXOUT_BASE_WEIGHT: u32 = 4 * core::mem::size_of::<u64>() as u32; // just the value

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
    return 9;
}

#[allow(unused)]
fn txout_weight_from_spk_len(spk_len: usize) -> u32 {
    (TXOUT_BASE_WEIGHT + varint_size(spk_len) + (spk_len as u32)) * 4
}
