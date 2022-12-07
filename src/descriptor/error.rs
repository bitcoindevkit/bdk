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

//! Descriptor errors

/// Errors related to the parsing and usage of descriptors
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid HD Key path, such as having a wildcard but a length != 1
    #[error("Invalid HD key path")]
    InvalidHdKeyPath,
    /// The provided descriptor doesn't match its checksum
    #[error("The provided descriptor doesn't match its checksum")]
    InvalidDescriptorChecksum,
    /// The descriptor contains hardened derivation steps on public extended keys
    #[error("The descriptor contains hardened derivation steps on public extended keys")]
    HardenedDerivationXpub,

    /// Error thrown while working with [`keys`](crate::keys)
    #[error("Key error: {0}")]
    Key(crate::keys::KeyError),
    /// Error while extracting and manipulating policies
    #[error("Policy error: {0}")]
    Policy(#[from] crate::descriptor::policy::PolicyError),

    /// Invalid byte found in the descriptor checksum
    #[error("Invalid descriptor character: {0}")]
    InvalidDescriptorCharacter(u8),

    /// BIP32 error
    #[error("BIP32 error: {0}")]
    Bip32(#[from] bitcoin::util::bip32::Error),
    /// Error during base58 decoding
    #[error("Base58 error: {0}")]
    Base58(#[from] bitcoin::util::base58::Error),
    /// Key-related error
    #[error("Key-related error: {0}")]
    Pk(#[from] bitcoin::util::key::Error),
    /// Miniscript error
    #[error("Miniscript error: {0}")]
    Miniscript(#[from] miniscript::Error),
    /// Hex decoding error
    #[error("Hex decoding error: {0}")]
    Hex(#[from] bitcoin::hashes::hex::Error),
}

impl From<crate::keys::KeyError> for Error {
    fn from(key_error: crate::keys::KeyError) -> Error {
        match key_error {
            crate::keys::KeyError::Miniscript(inner) => Error::Miniscript(inner),
            crate::keys::KeyError::Bip32(inner) => Error::Bip32(inner),
            e => Self::Key(e),
        }
    }
}
