use bincode::Options;
use std::{
    fs::File,
    io::{self, BufReader, Seek},
    marker::PhantomData,
};

use crate::bincode_options;

type EntryReader<'t> = CountingReader<BufReader<&'t mut File>>;

/// Iterator over entries in a file store.
///
/// Reads and returns an entry each time [`next`] is called. If an error occurs while reading the
/// iterator will yield a `Result::Err(_)` instead and then `None` for the next call to `next`.
///
/// [`next`]: Self::next
pub struct EntryIter<'t, T> {
    db_file: Option<EntryReader<'t>>,

    /// The file position for the first read of `db_file`.
    start_pos: Option<u64>,
    types: PhantomData<T>,
}

impl<'t, T> EntryIter<'t, T> {
    pub fn new(start_pos: u64, db_file: &'t mut File) -> Self {
        Self {
            db_file: Some(CountingReader::new(BufReader::new(db_file))),
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
        let read_one =
            |f: &mut EntryReader, start_pos: Option<u64>| -> Result<Option<T>, IterError> {
                if let Some(pos) = start_pos {
                    f.seek(io::SeekFrom::Start(pos))?;
                }
                match bincode_options().deserialize_from(&mut *f) {
                    Ok(changeset) => {
                        f.clear_count();
                        Ok(Some(changeset))
                    }
                    Err(e) => {
                        // allow unexpected EOF if 0 bytes were read
                        if let bincode::ErrorKind::Io(inner) = &*e {
                            if inner.kind() == io::ErrorKind::UnexpectedEof && f.count() == 0 {
                                f.clear_count();
                                return Ok(None);
                            }
                        }
                        f.rewind()?;
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

impl<'t, T> Drop for EntryIter<'t, T> {
    fn drop(&mut self) {
        if let Some(r) = self.db_file.as_mut() {
            // This syncs the underlying file's offset with the buffer's position. This way, no data
            // is lost with future reads.
            let _ = r.stream_position();
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

/// A wrapped [`Reader`] which counts total bytes read.
struct CountingReader<R> {
    r: R,
    n: u64,
}

impl<R> CountingReader<R> {
    fn new(file: R) -> Self {
        Self { r: file, n: 0 }
    }

    /// Counted bytes read.
    fn count(&self) -> u64 {
        self.n
    }

    /// Clear read count.
    fn clear_count(&mut self) {
        self.n = 0;
    }
}

impl<R: io::Seek> CountingReader<R> {
    /// Rewind file descriptor offset to before all counted read operations. Then clear the read
    /// count.
    fn rewind(&mut self) -> io::Result<u64> {
        let read = self.r.seek(std::io::SeekFrom::Current(-(self.n as i64)))?;
        self.n = 0;
        Ok(read)
    }
}

impl<R: io::Read> io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.r.read(&mut *buf)?;
        self.n += read as u64;
        Ok(read)
    }
}

impl<R: io::Seek> io::Seek for CountingReader<R> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        let res = self.r.seek(pos);
        self.n = 0;
        res
    }
}
