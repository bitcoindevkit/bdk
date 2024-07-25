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
use alloc::string::String;

use miniscript::descriptor::checksum::desc_checksum;

/// Compute the checksum of a descriptor, excludes any existing checksum in the descriptor string from the calculation
pub fn calc_checksum(desc: &str) -> Result<String, DescriptorError> {
    if let Some(split) = desc.split_once('#') {
        let og_checksum = split.1;
        let checksum = desc_checksum(split.0)?;
        if og_checksum != checksum {
            return Err(DescriptorError::InvalidDescriptorChecksum);
        }
        Ok(checksum)
    } else {
        Ok(desc_checksum(desc)?)
    }
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
        let sparkle_heart = unsafe { core::str::from_utf8_unchecked(&[240, 159, 146, 150]) };
        let invalid_desc = format!("wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcL{}fjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)", sparkle_heart);

        assert_matches!(
            calc_checksum(&invalid_desc),
            Err(DescriptorError::Miniscript(miniscript::Error::BadDescriptor(e))) if e == format!("Invalid character in checksum: '{}'", sparkle_heart)
        );
    }
}
