#![doc = include_str!("../README.md")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
mod entry_iter;
mod store;
use std::io;

use bincode::{DefaultOptions, Options};
pub use entry_iter::*;
pub use store::*;

pub(crate) fn bincode_options() -> impl bincode::Options {
    DefaultOptions::new().with_varint_encoding()
}

/// Error that occurs due to problems encountered with the file.
#[derive(Debug)]
pub enum StoreError {
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes { got: Vec<u8>, expected: Vec<u8> },
    /// Failure to decode data from the file.
    Bincode(bincode::ErrorKind),
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        fn fmt_hex_bytes(f: &mut core::fmt::Formatter<'_>, bytes: &[u8]) -> core::fmt::Result {
            for &b in bytes {
                write!(f, "{:02x}", b)?;
            }
            Ok(())
        }

        match self {
            Self::Io(e) => write!(f, "io error while reading store file: {}", e),
            Self::Bincode(e) => write!(f, "bincode error while decoding entry {}", e),
            Self::InvalidMagicBytes { got, expected } => {
                write!(f, "invalid magic bytes: ")?;
                write!(f, "expected 0x")?;
                fmt_hex_bytes(f, expected)?;
                write!(f, ", got 0x")?;
                fmt_hex_bytes(f, got)?;
                Ok(())
            }
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl std::error::Error for StoreError {}
