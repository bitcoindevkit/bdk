#![doc = include_str!("../README.md")]

mod store;

pub use store::*;

/// Error that occurs while appending new change sets to the DB.
#[derive(Debug)]
pub enum AppendError {
    /// JSON encoding error.
    Json(serde_json::Error),
    /// SQLite error.
    Sqlite(rusqlite::Error),
}

impl core::fmt::Display for AppendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "json error trying to append change set: {}", e),
            Self::Sqlite(e) => write!(f, "sqlite error trying to append change set: {}", e),
        }
    }
}

impl From<serde_json::Error> for AppendError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl std::error::Error for AppendError {}

/// Error the occurs while iterating stored change sets from the DB.
#[derive(Debug)]
pub enum IterError {
    /// Json decoding
    Json {
        rowid: usize,
        err: serde_json::Error,
    },
    /// Sqlite error
    Sqlite(rusqlite::Error),
    /// FromSql error
    FromSql(rusqlite::types::FromSqlError),
}

impl core::fmt::Display for IterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IterError::Json { rowid, err } => write!(
                f,
                "json error trying to decode change set {}, {}",
                rowid, err
            ),
            IterError::Sqlite(err) => write!(f, "Sqlite error {}", err),
            IterError::FromSql(err) => write!(f, "FromSql error {}", err),
        }
    }
}
