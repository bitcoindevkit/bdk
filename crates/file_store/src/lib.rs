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
pub enum FileError {
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes { got: Vec<u8>, expected: Vec<u8> },
}

impl core::fmt::Display for FileError {
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

impl From<io::Error> for FileError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl std::error::Error for FileError {}

/// An error while opening or creating the file store
#[derive(Debug)]
pub enum StoreError {
    /// Entry iter error
    EntryIter {
        /// Index that caused the error
        index: usize,
        /// Iter error
        iter: IterError,
        /// Amount of bytes read so far
        bytes_read: u64,
    },
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes { got: Vec<u8>, expected: Vec<u8> },
}

impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EntryIter {
                index,
                iter,
                bytes_read,
            } => write!(
                f,
                "{}: changeset index={}, bytes read={}",
                iter, index, bytes_read
            ),
            Self::Io(e) => write!(f, "io error trying to read file: {}", e),
            Self::InvalidMagicBytes { got, expected } => write!(
                f,
                "file has invalid magic bytes: expected={:?} got={:?}",
                expected, got,
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
