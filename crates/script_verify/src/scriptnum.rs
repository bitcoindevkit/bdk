// Adapted from rbitcoin `crates/rbitcoin-primitives/src/scriptnum.rs` at
// 49210ea82130474a4df366e253d1346a16369fdb (MIT OR Apache-2.0, rbitcoin contributors).

use alloc::vec::Vec;

use crate::Fail;

pub fn encode(mut n: i64) -> Vec<u8> {
    if n == 0 {
        return Vec::new();
    }
    let neg = n < 0;
    if neg {
        n = -n;
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push((n & 0xff) as u8);
        n >>= 8;
    }
    if out.last().map(|b| b & 0x80 != 0).unwrap_or(false) {
        out.push(if neg { 0x80 } else { 0x00 });
    } else if neg {
        let last = out.last_mut().expect("n != 0 so out is non-empty");
        *last |= 0x80;
    }
    out
}

pub fn decode(v: &[u8]) -> Result<i64, Fail> {
    decode_width(v, 4)
}

/// `max_len` is 4 for arithmetic. CLTV and CSV use 5 so that every `u32` encodes as positive.
pub fn decode_width(v: &[u8], max_len: usize) -> Result<i64, Fail> {
    if v.len() > max_len {
        return Err(Fail::Script("SCRIPTNUM_OVERFLOW"));
    }
    let Some(&last) = v.last() else {
        return Ok(0);
    };
    let mut result: i64 = 0;
    for (i, &b) in v.iter().enumerate() {
        result |= (b as i64) << (8 * i);
    }
    if last & 0x80 != 0 {
        result &= !(0x80i64 << (8 * (v.len() - 1)));
        result = -result;
    }
    Ok(result)
}
