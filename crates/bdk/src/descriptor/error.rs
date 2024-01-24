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
use core::fmt;

/// Errors related to the parsing and usage of descriptors
#[derive(Debug)]
pub enum Error {
    /// Invalid HD Key path, such as having a wildcard but a length != 1
    InvalidHdKeyPath,
    /// The provided descriptor doesn't match its checksum
    InvalidDescriptorChecksum,
    /// The descriptor contains hardened derivation steps on public extended keys
    HardenedDerivationXpub,
    /// The descriptor contains multipath keys
    MultiPath,

    /// Error thrown while working with [`keys`](crate::keys)
    Key(crate::keys::KeyError),
    /// Error while extracting and manipulating policies
    Policy(crate::descriptor::policy::PolicyError),

    /// Invalid byte found in the descriptor checksum
    InvalidDescriptorCharacter(u8),

    /// BIP32 error
    Bip32(bitcoin::bip32::Error),
    /// Error during base58 decoding
    Base58(bitcoin::base58::Error),
    /// Key-related error
    Pk(bitcoin::key::Error),
    /// Miniscript error
    Miniscript(miniscript::Error),
    /// Hex Array decoding error
    HexToArray(bitcoin::hex::HexToArrayError),
    /// Hex Bytes decoding error
    HexToBytes(bitcoin::hex::HexToBytesError),
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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHdKeyPath => write!(f, "Invalid HD key path"),
            Self::InvalidDescriptorChecksum => {
                write!(f, "The provided descriptor doesn't match its checksum")
            }
            Self::HardenedDerivationXpub => write!(
                f,
                "The descriptor contains hardened derivation steps on public extended keys"
            ),
            Self::MultiPath => write!(
                f,
                "The descriptor contains multipath keys, which are not supported yet"
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
            Self::HexToArray(err) => write!(f, "HexToArray decoding error: {}", err),
            Self::HexToBytes(err) => write!(f, "HexToBytes decoding error: {}", err),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

impl From<bitcoin::bip32::Error> for Error {
    fn from(err: bitcoin::bip32::Error) -> Self {
        Error::Bip32(err)
    }
}

impl From<bitcoin::base58::Error> for Error {
    fn from(err: bitcoin::base58::Error) -> Self {
        Error::Base58(err)
    }
}

impl From<bitcoin::key::Error> for Error {
    fn from(err: bitcoin::key::Error) -> Self {
        Error::Pk(err)
    }
}

impl From<miniscript::Error> for Error {
    fn from(err: miniscript::Error) -> Self {
        Error::Miniscript(err)
    }
}

impl From<bitcoin::hex::HexToArrayError> for Error {
    fn from(err: bitcoin::hex::HexToArrayError) -> Self {
        Error::HexToArray(err)
    }
}

impl From<bitcoin::hex::HexToBytesError> for Error {
    fn from(err: bitcoin::hex::HexToBytesError) -> Self {
        Error::HexToBytes(err)
    }
}

impl From<crate::descriptor::policy::PolicyError> for Error {
    fn from(err: crate::descriptor::policy::PolicyError) -> Self {
        Error::Policy(err)
    }
}
