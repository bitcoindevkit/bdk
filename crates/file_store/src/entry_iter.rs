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
pub struct EntryIter<'t, T1, T2 = T1> {
    /// Buffered reader around the file
    db_file: BufReader<&'t mut File>,
    finished: bool,
    /// The file position for the first read of `db_file`.
    start_pos: Option<u64>,
    types: PhantomData<(T1, T2)>,
}

impl<'t, T1, T2> EntryIter<'t, T1, T2> {
    pub fn new(start_pos: u64, db_file: &'t mut File) -> Self {
        Self {
            db_file: BufReader::new(db_file),
            start_pos: Some(start_pos),
            finished: false,
            types: PhantomData,
        }
    }
}

impl<'t, T1, T2> Iterator for EntryIter<'t, T1, T2>
where
    T1: serde::de::DeserializeOwned + Default + serde::Serialize,
    T2: serde::de::DeserializeOwned + Into<T1>,
{
    type Item = Result<T1, IterError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        (|| {
            if let Some(start) = self.start_pos.take() {
                self.db_file.seek(io::SeekFrom::Start(start))?;
            }

            let mut pos_before_read = self.db_file.stream_position()?;
            bincode_options()
                .deserialize_from::<_, u64>(&mut self.db_file)
                .and_then(|size| {
                    pos_before_read = self.db_file.stream_position()?;
                    bincode_options()
                        .with_limit(size)
                        .deserialize_from::<_, T1>(&mut self.db_file)
                })
                .or_else(|e| {
                    let serialized_max_variant = bincode_options().serialize(&T1::default())?;
                    // allow trailing bytes because we are serializing the variant, not an u64, and
                    // we want to extract the varint index prefixed
                    let max_variant_index = bincode_options()
                        .allow_trailing_bytes()
                        .deserialize::<u64>(&serialized_max_variant)?;

                    match &*e {
                        bincode::ErrorKind::Custom(e) if e.contains("expected variant index") => {
                            self.db_file.seek(io::SeekFrom::Start(pos_before_read))?;
                            pos_before_read = self.db_file.stream_position()?;
                            let actual_index =
                                bincode_options().deserialize_from::<_, u64>(&mut self.db_file)?;
                            // This will always happen as the `expected variant index` only will be
                            // risen when actual_index > max_variant_index
                            if actual_index > max_variant_index {
                                pos_before_read = self.db_file.stream_position()?;
                                return bincode_options()
                                    .deserialize_from::<_, T2>(&mut self.db_file)
                                    .map(Into::into);
                            }
                        }
                        bincode::ErrorKind::InvalidTagEncoding(index) => {
                            if *index as u64 > max_variant_index {
                                pos_before_read = self.db_file.stream_position()?;
                                return bincode_options()
                                    .deserialize_from::<_, T2>(&mut self.db_file)
                                    .map(Into::into);
                            }
                        }
                        _ => (),
                    };

                    Err(e)
                })
                .map(|changeset| Some(changeset))
                .or_else(|e| {
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
                })
        })()
        .transpose()
    }
}

impl<'t, T1, T2> Drop for EntryIter<'t, T1, T2> {
    fn drop(&mut self) {
        // This syncs the underlying file's offset with the buffer's position. This way, we
        // maintain the correct position to start the next read/write.
        if let Ok(pos) = self.db_file.stream_position() {
            let _ = self.db_file.get_mut().seek(io::SeekFrom::Start(pos));
        }
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
