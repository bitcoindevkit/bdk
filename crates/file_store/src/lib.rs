#![doc = include_str!("../README.md")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
mod entry_iter;
mod store;
use std::io;

pub use entry_iter::*;
pub use store::*;

/// Error that occurs due to problems encountered with the file.
#[derive(Debug)]
pub enum StoreError {
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes { got: Vec<u8>, expected: Vec<u8> },
    /// Failure to decode an entry from the file.
    Decode(postcard::Error),
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
            Self::Decode(e) => write!(f, "error while decoding store entry: {}", e),
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

impl core::error::Error for StoreError {}
