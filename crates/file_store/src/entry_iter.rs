use bincode::Options;
use std::{
    fs::File,
    io::{self, BufReader, Seek},
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
    /// Buffered reader around the file
    db_file: BufReader<&'t mut File>,
    finished: bool,
    /// The file position for the first read of `db_file`.
    start_pos: Option<u64>,
    types: PhantomData<T>,
}

impl<'t, T> EntryIter<'t, T> {
    pub fn new(start_pos: u64, db_file: &'t mut File) -> Self {
        Self {
            db_file: BufReader::new(db_file),
            start_pos: Some(start_pos),
            finished: false,
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
        if self.finished {
            return None;
        }
        (|| {
            if let Some(start) = self.start_pos.take() {
                self.db_file.seek(io::SeekFrom::Start(start))?;
            }

            let pos_before_read = self.db_file.stream_position()?;
            match bincode_options().deserialize_from(&mut self.db_file) {
                Ok(changeset) => Ok(Some(changeset)),
                Err(e) => {
                    self.finished = true;
                    let pos_after_read = self.db_file.stream_position()?;
                    // allow unexpected EOF if 0 bytes were read
                    if let bincode::ErrorKind::Io(inner) = &*e {
                        if inner.kind() == io::ErrorKind::UnexpectedEof
                            && pos_after_read == pos_before_read
                        {
                            return Ok(None);
                        }
                    }
                    self.db_file.seek(io::SeekFrom::Start(pos_before_read))?;
                    Err(IterError::Bincode(*e))
                }
            }
        })()
        .transpose()
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

impl From<io::Error> for IterError {
    fn from(value: io::Error) -> Self {
        IterError::Io(value)
    }
}

impl std::error::Error for IterError {}
