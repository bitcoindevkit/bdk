use bincode::Options;
use std::{
    fs::File,
    io::{self, Seek},
    marker::PhantomData,
};

use crate::bincode_options;

/// Iterator over entries in a file store.
///
/// Reads and returns an entry each time [`next`] is called. If an error occurs while reading the
/// iterator will yield a `Result::Err(_)` instead and then `None` for the next call to `next`.
///
/// [`next`]: Self::next
pub struct EntryIter<'a, V> {
    db_file: &'a mut File,
    types: PhantomData<V>,
    error_exit: bool,
}

impl<'a, V> EntryIter<'a, V> {
    pub fn new(db_file: &'a mut File) -> Self {
        Self {
            db_file,
            types: PhantomData,
            error_exit: false,
        }
    }
}

impl<'a, V> Iterator for EntryIter<'a, V>
where
    V: serde::de::DeserializeOwned,
{
    type Item = Result<V, IterError>;

    fn next(&mut self) -> Option<Self::Item> {
        let result = (|| {
            let pos = self.db_file.stream_position()?;

            match bincode_options().deserialize_from(&mut self.db_file) {
                Ok(changeset) => Ok(Some(changeset)),
                Err(e) => {
                    if let bincode::ErrorKind::Io(inner) = &*e {
                        if inner.kind() == io::ErrorKind::UnexpectedEof {
                            let eof = self.db_file.seek(io::SeekFrom::End(0))?;
                            if pos == eof {
                                return Ok(None);
                            }
                        }
                    }

                    self.db_file.seek(io::SeekFrom::Start(pos))?;
                    Err(IterError::Bincode(*e))
                }
            }
        })();

        let result = result.transpose();

        if let Some(Err(_)) = &result {
            self.error_exit = true;
        }

        result
    }
}

impl From<io::Error> for IterError {
    fn from(value: io::Error) -> Self {
        IterError::Io(value)
    }
}

/// Error type for [`EntryIter`].
#[derive(Debug)]
pub enum IterError {
    /// Failure to read from the file.
    Io(io::Error),
    /// Failure to decode data from the file.
    Bincode(bincode::ErrorKind),
}

impl core::fmt::Display for IterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IterError::Io(e) => write!(f, "io error trying to read entry {}", e),
            IterError::Bincode(e) => write!(f, "bincode error while reading entry {}", e),
        }
    }
}

impl std::error::Error for IterError {}
