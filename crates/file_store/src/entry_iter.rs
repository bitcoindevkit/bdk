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
pub struct EntryIter<'t, T> {
    db_file: Option<&'t mut File>,

    /// The file position for the first read of `db_file`.
    start_pos: Option<u64>,
    types: PhantomData<T>,
}

impl<'t, T> EntryIter<'t, T> {
    pub fn new(start_pos: u64, db_file: &'t mut File) -> Self {
        Self {
            db_file: Some(db_file),
            start_pos: Some(start_pos),
            types: PhantomData,
        }
    }
}

impl<'t, T> Iterator for EntryIter<'t, T>
where
    T: serde::de::DeserializeOwned,
{
    type Item = Result<T, IterError>;

    fn next(&mut self) -> Option<Self::Item> {
        // closure which reads a single entry starting from `self.pos`
        let read_one = |f: &mut File, start_pos: Option<u64>| -> Result<Option<T>, IterError> {
            let pos = match start_pos {
                Some(pos) => f.seek(io::SeekFrom::Start(pos))?,
                None => f.stream_position()?,
            };

            match bincode_options().deserialize_from(&*f) {
                Ok(changeset) => {
                    f.stream_position()?;
                    Ok(Some(changeset))
                }
                Err(e) => {
                    if let bincode::ErrorKind::Io(inner) = &*e {
                        if inner.kind() == io::ErrorKind::UnexpectedEof {
                            let eof = f.seek(io::SeekFrom::End(0))?;
                            if pos == eof {
                                return Ok(None);
                            }
                        }
                    }
                    f.seek(io::SeekFrom::Start(pos))?;
                    Err(IterError::Bincode(*e))
                }
            }
        };

        let result = read_one(self.db_file.as_mut()?, self.start_pos.take());
        if result.is_err() {
            self.db_file = None;
        }
        result.transpose()
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
