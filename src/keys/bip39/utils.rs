// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2022 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bitcoin::hashes::{sha256, Hash};

pub(crate) const U8_BITS: usize = u8::BITS as _;
pub(crate) const U11_BITS: usize = 11;
pub(crate) const U11_MAX: u16 = 2047;
pub(crate) const U11_EOF: u16 = u16::MAX;

pub(crate) fn u8_to_bool_array(v: &u8) -> [bool; U8_BITS] {
    let mut bits = [false; U8_BITS];
    bits.iter_mut()
        .enumerate()
        .for_each(|(i, b)| *b = v & (1 << (U8_BITS - 1 - i)) != 0);
    bits
}

pub(crate) fn u11_to_bool_array(v: &u16) -> [bool; U11_BITS] {
    debug_assert!(*v <= U11_MAX);
    let mut bits = [false; U11_BITS];
    bits.iter_mut()
        .enumerate()
        .for_each(|(i, b)| *b = v & (1 << (U11_BITS - 1 - i)) != 0);
    bits
}

pub(crate) fn bool_array_to_u11(arr: &[bool]) -> u16 {
    debug_assert!(arr.len() == U11_BITS);
    arr.iter().enumerate().fold(0_u16, |out, (i, b)| {
        out + match b {
            true => 1 << (U11_BITS - 1 - i),
            false => 0_u16,
        }
    })
}

pub(crate) fn bool_array_to_u8(arr: &[bool]) -> u8 {
    debug_assert!(arr.len() == U8_BITS);
    arr.iter().enumerate().fold(0_u8, |out, (i, b)| {
        out + match b {
            true => 1 << (U8_BITS - 1 - i),
            false => 0_u8,
        }
    })
}

/// Returns the first byte of the entropy hash.
///
/// The actual size for CS should be ENT/32bits and is handled elsewhere.
pub(crate) fn generate_checksum_byte(entropy: &[u8]) -> u8 {
    sha256::Hash::hash(entropy)[0]
}
