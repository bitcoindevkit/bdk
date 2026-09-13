use crate::StoreError;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek},
    marker::PhantomData,
};

/// Iterator over entries in a file store.
///
/// Reads and returns an entry each time [`next`] is called. If an error occurs while reading the
/// iterator will yield a `Result::Err(_)` instead and then `None` for the next call to `next`.
///
/// Each entry is stored as a `postcard`-encoded `u64` varint length prefix followed by that many
/// bytes of `postcard`-encoded data.
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
        match self.read_entry() {
            Ok(entry) => entry.map(Ok),
            Err(e) => {
                self.finished = true;
                Some(Err(e))
            }
        }
    }
}

impl<T> EntryIter<'_, T>
where
    T: serde::de::DeserializeOwned,
{
    /// Reads the next entry, or `Ok(None)` on clean end-of-file.
    ///
    /// On error the file is rewound to the start of the failed entry, so it isn't left mid-entry.
    fn read_entry(&mut self) -> Result<Option<T>, StoreError> {
        if let Some(start) = self.start_pos.take() {
            self.db_file.seek(io::SeekFrom::Start(start))?;
        }
        let pos_before_read = self.db_file.stream_position()?;

        // An empty buffer here is a clean end-of-file, not a torn entry. Done before the rewind
        // scope below because a failed peek consumes nothing.
        if self.db_file.fill_buf()?.is_empty() {
            return Ok(None);
        }

        let entry = self.read_frame();
        if entry.is_err() {
            // Leave the file at the start of the failed entry.
            self.db_file.seek(io::SeekFrom::Start(pos_before_read))?;
        }
        entry.map(Some)
    }

    /// Reads a single frame.
    ///
    /// A frame is a `postcard` varint length prefix followed by that many bytes of
    /// `postcard`-encoded data.
    fn read_frame(&mut self) -> Result<T, StoreError> {
        let len = self.read_len_prefix()?;
        let payload_start = self.db_file.stream_position()?;
        let payload = self.read_payload(len, payload_start)?;
        decode_frame(&payload)
    }

    /// Reads the frame length prefix.
    ///
    /// The varint length prefix is a `postcard`-encoded `u64`, at most 10 bytes, where the high bit
    /// of each byte (0x80 mask) is the continuation flag.
    fn read_len_prefix(&mut self) -> Result<u64, StoreError> {
        let mut buf = [0_u8; 10];

        for (i, byte) in buf.iter_mut().enumerate() {
            if self.db_file.read(std::slice::from_mut(byte))? == 0 {
                // Prefix cut short by end-of-file: a torn entry.
                return Err(StoreError::Decode(
                    postcard::Error::DeserializeUnexpectedEnd,
                ));
            }

            if *byte & 0x80 == 0 {
                return postcard::from_bytes(&buf[..=i]).map_err(StoreError::Decode);
            }
        }

        // Continuation flag still set after 10 bytes: not a valid u64 varint.
        Err(StoreError::Decode(
            postcard::Error::DeserializeUnexpectedEnd,
        ))
    }

    /// Reads `len` payload bytes into a fresh buffer.
    fn read_payload(&mut self, len: u64, payload_start: u64) -> Result<Vec<u8>, StoreError> {
        let mut payload = Vec::new();
        // Reserve exactly `len` bytes up front. Fail fast on a corrupt, oversized length prefix.
        // Avoids unnecessary reads and allocations.
        let alloc_failed = match usize::try_from(len) {
            Ok(len) => payload.try_reserve_exact(len).is_err(),
            Err(_) => true,
        };
        if alloc_failed {
            return Err(self.alloc_failure_error(len, payload_start));
        }

        let bytes_read = (&mut self.db_file).take(len).read_to_end(&mut payload)?;
        if bytes_read as u64 != len {
            return Err(StoreError::Decode(
                postcard::Error::DeserializeUnexpectedEnd,
            ));
        }
        Ok(payload)
    }

    /// Discover the kind of allocation error.
    ///
    /// This only runs after `len` has already failed to allocate, so it is a big number but not
    /// necessarily corrupt. Here we distinguish whether it exceeds the bytes actually remaining
    /// in the file (a decode error), or if it fits within the file but exceeds what this
    /// machine can allocate right now (an environment failure, not a format one).
    fn alloc_failure_error(&self, len: u64, payload_start: u64) -> StoreError {
        let remaining = self
            .db_file
            .get_ref()
            .metadata()
            .map(|m| m.len().saturating_sub(payload_start))
            .unwrap_or(0);
        if len > remaining {
            StoreError::Decode(postcard::Error::DeserializeUnexpectedEnd)
        } else {
            StoreError::Io(io::Error::other("failed to allocate memory for entry"))
        }
    }
}

/// Decodes one framed payload.
///
/// The length prefix stays authoritative for framing, so bytes left over after decoding are
/// corruption, not a format `postcard` has a dedicated variant for.
fn decode_frame<T: serde::de::DeserializeOwned>(payload: &[u8]) -> Result<T, StoreError> {
    match postcard::take_from_bytes(payload) {
        Ok((changeset, [])) => Ok(changeset),
        Ok(_) => Err(StoreError::Decode(postcard::Error::SerdeDeCustom)),
        Err(e) => Err(StoreError::Decode(e)),
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use super::*;

    // A single `0x80` byte is a varint continuation flag with no terminating byte: the length
    // prefix is cut short by end-of-file, i.e. a torn entry.
    fn torn_prefix_file() -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(file.as_file_mut(), &[0x80]).unwrap();
        file
    }

    // The iterator yields the error for a torn length prefix, then is fused: subsequent calls to
    // `next` return `None` (and the file is rewound to the start of the failed entry).
    #[test]
    fn next_returns_none_after_error() {
        let mut file = torn_prefix_file();
        let mut iter = EntryIter::<String>::new(0, file.as_file_mut());

        match iter.next() {
            Some(Err(StoreError::Decode(postcard::Error::DeserializeUnexpectedEnd))) => {}
            unexpected => panic!("unexpected result: {unexpected:?}"),
        }
        assert_eq!(iter.db_file.stream_position().unwrap(), 0);
        // subsequent calls to `next` return `None`
        assert!(iter.next().is_none());
        // check twice
        assert!(iter.next().is_none());
    }

    // A length prefix cut short by end-of-file is a torn entry, not a clean end-of-file.
    #[test]
    fn errors_on_truncated_length_prefix() {
        let mut file = torn_prefix_file();
        let mut iter = EntryIter::<String>::new(0, file.as_file_mut());

        match iter.next() {
            Some(Err(StoreError::Decode(postcard::Error::DeserializeUnexpectedEnd))) => {}
            unexpected => panic!("unexpected result: {unexpected:?}"),
        }
    }

    // Ten bytes with the continuation flag still set is not a valid `u64` varint.
    #[test]
    fn errors_on_overlong_length_prefix() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(file.as_file_mut(), &[0xFF; 10]).unwrap();
        let mut iter = EntryIter::<String>::new(0, file.as_file_mut());

        match iter.next() {
            Some(Err(StoreError::Decode(postcard::Error::DeserializeUnexpectedEnd))) => {}
            unexpected => panic!("unexpected result: {unexpected:?}"),
        }
    }

    // A length prefix that fits within the file but is too large to allocate is an environment
    // failure (`Io`), not a format one (`Decode`).
    //
    // `try_reserve_exact` allocates the full requested size, so a length larger than the
    // machine's available memory fails. A sparse file of 1 TiB makes such a length fit within the
    // file while still exceeding what any reasonable machine can allocate. Skipped on filesystems
    // that cannot create large sparse files (e.g. tmpfs).
    #[test]
    fn errors_with_io_when_length_fits_but_allocation_fails() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let file_size = 1u64 << 40; // 1 TiB
        assert!(
            file.as_file_mut().set_len(file_size).is_ok(),
            "Filesystem can't create a large sparse file (e.g. tmpfs); can't exercise this path."
        );

        // A length that fits within the file but is far too large to allocate.
        let len = file_size - 1000;
        std::io::Write::write_all(file.as_file_mut(), &postcard::to_allocvec(&len).unwrap())
            .unwrap();

        let mut iter = EntryIter::<String>::new(0, file.as_file_mut());
        match iter.next() {
            Some(Err(StoreError::Io(_))) => {}
            unexpected => panic!("unexpected result: {unexpected:?}"),
        }
    }
}
