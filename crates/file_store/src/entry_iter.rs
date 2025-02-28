use crate::StoreError;
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

impl<T> Iterator for EntryIter<'_, T>
where
    T: serde::de::DeserializeOwned,
{
    type Item = Result<T, StoreError>;

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
                    Err(StoreError::Bincode(*e))
                }
            }
        })()
        .transpose()
    }
}

impl<T> Drop for EntryIter<'_, T> {
    fn drop(&mut self) {
        // This syncs the underlying file's offset with the buffer's position. This way, we
        // maintain the correct position to start the next read/write.
        if let Ok(pos) = self.db_file.stream_position() {
            let _ = self.db_file.get_mut().seek(io::SeekFrom::Start(pos));
        }
    }
}
