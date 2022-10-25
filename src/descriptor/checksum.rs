// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2022 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Descriptor checksum
//!
//! This module contains a re-implementation of the function used by Bitcoin Core to calculate the
//! checksum of a descriptor

use crate::descriptor::DescriptorError;

const INPUT_CHARSET: &[u8] = b"0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
const CHECKSUM_CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

fn poly_mod(mut c: u64, val: u64) -> u64 {
    let c0 = c >> 35;
    c = ((c & 0x7ffffffff) << 5) ^ val;
    if c0 & 1 > 0 {
        c ^= 0xf5dee51989
    };
    if c0 & 2 > 0 {
        c ^= 0xa9fdca3312
    };
    if c0 & 4 > 0 {
        c ^= 0x1bab10e32d
    };
    if c0 & 8 > 0 {
        c ^= 0x3706b1677a
    };
    if c0 & 16 > 0 {
        c ^= 0x644d626ffd
    };

    c
}

/// Computes the checksum bytes of a descriptor
pub fn get_checksum_bytes(desc: &str) -> Result<[u8; 8], DescriptorError> {
    let mut c = 1;
    let mut cls = 0;
    let mut clscount = 0;

    for ch in desc.as_bytes() {
        let pos = INPUT_CHARSET
            .iter()
            .position(|b| b == ch)
            .ok_or(DescriptorError::InvalidDescriptorCharacter(*ch))? as u64;
        c = poly_mod(c, pos & 31);
        cls = cls * 3 + (pos >> 5);
        clscount += 1;
        if clscount == 3 {
            c = poly_mod(c, cls);
            cls = 0;
            clscount = 0;
        }
    }
    if clscount > 0 {
        c = poly_mod(c, cls);
    }
    (0..8).for_each(|_| c = poly_mod(c, 0));
    c ^= 1;

    let mut checksum = [0_u8; 8];
    for j in 0..8 {
        checksum[j] = CHECKSUM_CHARSET[((c >> (5 * (7 - j))) & 31) as usize];
    }

    Ok(checksum)
}

/// Compute the checksum of a descriptor
pub fn get_checksum(desc: &str) -> Result<String, DescriptorError> {
    // unsafe is okay here as the checksum only uses bytes in `CHECKSUM_CHARSET`
    get_checksum_bytes(desc).map(|b| unsafe { String::from_utf8_unchecked(b.to_vec()) })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::descriptor::get_checksum;

    // test get_checksum() function; it should return the same value as Bitcoin Core
    #[test]
    fn test_get_checksum() {
        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)";
        assert_eq!(get_checksum(desc).unwrap(), "tqz0nc62");

        let desc = "pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44'/1'/0'/0/*)";
        assert_eq!(get_checksum(desc).unwrap(), "lasegmfs");
    }

    #[test]
    fn test_get_checksum_invalid_character() {
        let sparkle_heart = unsafe { std::str::from_utf8_unchecked(&[240, 159, 146, 150]) };
        let invalid_desc = format!("wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcL{}fjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)", sparkle_heart);

        assert!(matches!(
            get_checksum(&invalid_desc).err(),
            Some(DescriptorError::InvalidDescriptorCharacter(invalid_char)) if invalid_char == sparkle_heart.as_bytes()[0]
        ));
    }
}
