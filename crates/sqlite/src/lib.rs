#![doc = include_str!("../README.md")]
// only enables the `doc_cfg` feature when the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod schema;
mod store;

use bdk_chain::bitcoin::Network;
pub use rusqlite;
pub use store::Store;

/// Error that occurs while reading or writing change sets with the SQLite database.
#[derive(Debug)]
pub enum Error {
    /// Invalid network, cannot change the one already stored in the database.
    Network { expected: Network, given: Network },
    /// SQLite error.
    Sqlite(rusqlite::Error),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Network { expected, given } => write!(
                f,
                "network error trying to read or write change set, expected {}, given {}",
                expected, given
            ),
            Self::Sqlite(e) => write!(f, "sqlite error reading or writing changeset: {}", e),
        }
    }
}

impl std::error::Error for Error {}
