// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
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

/// Computes the checksum bytes of a descriptor.
/// `exclude_hash = true` ignores all data after the first '#' (inclusive).
pub(crate) fn calc_checksum_bytes_internal(
    mut desc: &str,
    exclude_hash: bool,
) -> Result<[u8; 8], DescriptorError> {
    let mut c = 1;
    let mut cls = 0;
    let mut clscount = 0;

    let mut original_checksum = None;
    if exclude_hash {
        if let Some(split) = desc.split_once('#') {
            desc = split.0;
            original_checksum = Some(split.1);
        }
    }

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

    // if input data already had a checksum, check calculated checksum against original checksum
    if let Some(original_checksum) = original_checksum {
        if original_checksum.as_bytes() != checksum {
            return Err(DescriptorError::InvalidDescriptorChecksum);
        }
    }

    Ok(checksum)
}

/// Compute the checksum bytes of a descriptor, excludes any existing checksum in the descriptor string from the calculation
pub fn calc_checksum_bytes(desc: &str) -> Result<[u8; 8], DescriptorError> {
    calc_checksum_bytes_internal(desc, true)
}

/// Compute the checksum of a descriptor, excludes any existing checksum in the descriptor string from the calculation
pub fn calc_checksum(desc: &str) -> Result<String, DescriptorError> {
    // unsafe is okay here as the checksum only uses bytes in `CHECKSUM_CHARSET`
    calc_checksum_bytes_internal(desc, true)
        .map(|b| unsafe { String::from_utf8_unchecked(b.to_vec()) })
}

// TODO in release 0.25.0, remove get_checksum_bytes and get_checksum
// TODO in release 0.25.0, consolidate calc_checksum_bytes_internal into calc_checksum_bytes

/// Compute the checksum bytes of a descriptor
#[deprecated(
    since = "0.24.0",
    note = "Use new `calc_checksum_bytes` function which excludes any existing checksum in the descriptor string before calculating the checksum hash bytes. See https://github.com/bitcoindevkit/bdk/pull/765."
)]
pub fn get_checksum_bytes(desc: &str) -> Result<[u8; 8], DescriptorError> {
    calc_checksum_bytes_internal(desc, false)
}

/// Compute the checksum of a descriptor
#[deprecated(
    since = "0.24.0",
    note = "Use new `calc_checksum` function which excludes any existing checksum in the descriptor string before calculating the checksum hash. See https://github.com/bitcoindevkit/bdk/pull/765."
)]
pub fn get_checksum(desc: &str) -> Result<String, DescriptorError> {
    // unsafe is okay here as the checksum only uses bytes in `CHECKSUM_CHARSET`
    calc_checksum_bytes_internal(desc, false)
        .map(|b| unsafe { String::from_utf8_unchecked(b.to_vec()) })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::descriptor::calc_checksum;
    use assert_matches::assert_matches;

    // test calc_checksum() function; it should return the same value as Bitcoin Core
    #[test]
    fn test_calc_checksum() {
        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)";
        assert_eq!(calc_checksum(desc).unwrap(), "tqz0nc62");

        let desc = "pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44'/1'/0'/0/*)";
        assert_eq!(calc_checksum(desc).unwrap(), "lasegmfs");
    }

    // test calc_checksum() function; it should return the same value as Bitcoin Core even if the
    // descriptor string includes a checksum hash
    #[test]
    fn test_calc_checksum_with_checksum_hash() {
        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)#tqz0nc62";
        assert_eq!(calc_checksum(desc).unwrap(), "tqz0nc62");

        let desc = "pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44'/1'/0'/0/*)#lasegmfs";
        assert_eq!(calc_checksum(desc).unwrap(), "lasegmfs");

        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)#tqz0nc26";
        assert_matches!(
            calc_checksum(desc),
            Err(DescriptorError::InvalidDescriptorChecksum)
        );

        let desc = "pkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/44'/1'/0'/0/*)#lasegmsf";
        assert_matches!(
            calc_checksum(desc),
            Err(DescriptorError::InvalidDescriptorChecksum)
        );
    }

    #[test]
    fn test_calc_checksum_invalid_character() {
        let sparkle_heart = unsafe { std::str::from_utf8_unchecked(&[240, 159, 146, 150]) };
        let invalid_desc = format!("wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcL{}fjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)", sparkle_heart);

        assert_matches!(
            calc_checksum(&invalid_desc),
            Err(DescriptorError::InvalidDescriptorCharacter(invalid_char)) if invalid_char == sparkle_heart.as_bytes()[0]
        );
    }
}
