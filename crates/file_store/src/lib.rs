#![doc = include_str!("../README.md")]
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
pub enum FileError<'a> {
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes { got: Vec<u8>, expected: &'a [u8] },
}

impl<'a> core::fmt::Display for FileError<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error trying to read file: {}", e),
            Self::InvalidMagicBytes { got, expected } => write!(
                f,
                "file has invalid magic bytes: expected={:?} got={:?}",
                expected, got,
            ),
        }
    }
}

impl<'a> From<io::Error> for FileError<'a> {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl<'a> std::error::Error for FileError<'a> {}
