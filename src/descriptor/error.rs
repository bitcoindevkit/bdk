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
#[derive(Debug)]
pub enum Error {
    /// Invalid HD Key path, such as having a wildcard but a length != 1
    InvalidHdKeyPath,
    /// The provided descriptor doesn't match its checksum
    InvalidDescriptorChecksum,
    /// The descriptor contains hardened derivation steps on public extended keys
    HardenedDerivationXpub,

    /// Error thrown while working with [`keys`](crate::keys)
    Key(crate::keys::KeyError),
    /// Error while extracting and manipulating policies
    Policy(crate::descriptor::policy::PolicyError),

    /// Invalid byte found in the descriptor checksum
    InvalidDescriptorCharacter(u8),

    /// BIP32 error
    Bip32(bitcoin::util::bip32::Error),
    /// Error during base58 decoding
    Base58(bitcoin::util::base58::Error),
    /// Key-related error
    Pk(bitcoin::util::key::Error),
    /// Miniscript error
    Miniscript(miniscript::Error),
    /// Hex decoding error
    Hex(bitcoin::hashes::hex::Error),
}

impl From<crate::keys::KeyError> for Error {
    fn from(key_error: crate::keys::KeyError) -> Error {
        match key_error {
            crate::keys::KeyError::Miniscript(inner) => Error::Miniscript(inner),
            crate::keys::KeyError::Bip32(inner) => Error::Bip32(inner),
            e => Error::Key(e),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHdKeyPath => write!(f, "Invalid HD key path"),
            Self::InvalidDescriptorChecksum => {
                write!(f, "The provided descriptor doesn't match its checksum")
            }
            Self::HardenedDerivationXpub => write!(
                f,
                "The descriptor contains hardened derivation steps on public extended keys"
            ),
            Self::Key(err) => write!(f, "Key error: {}", err),
            Self::Policy(err) => write!(f, "Policy error: {}", err),
            Self::InvalidDescriptorCharacter(char) => {
                write!(f, "Invalid descriptor character: {}", char)
            }
            Self::Bip32(err) => write!(f, "BIP32 error: {}", err),
            Self::Base58(err) => write!(f, "Base58 error: {}", err),
            Self::Pk(err) => write!(f, "Key-related error: {}", err),
            Self::Miniscript(err) => write!(f, "Miniscript error: {}", err),
            Self::Hex(err) => write!(f, "Hex decoding error: {}", err),
        }
    }
}

impl std::error::Error for Error {}

impl_error!(bitcoin::util::bip32::Error, Bip32);
impl_error!(bitcoin::util::base58::Error, Base58);
impl_error!(bitcoin::util::key::Error, Pk);
impl_error!(miniscript::Error, Miniscript);
impl_error!(bitcoin::hashes::hex::Error, Hex);
impl_error!(crate::descriptor::policy::PolicyError, Policy);
