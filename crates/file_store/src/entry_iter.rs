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
        let entry = self.read_entry().transpose();
        // stop after the end of the file or the first error
        if !matches!(entry, Some(Ok(_))) {
            self.finished = true;
        }
        entry
    }
}

impl<T> EntryIter<'_, T>
where
    T: serde::de::DeserializeOwned,
{
    /// Reads the next entry, or `Ok(None)` on a clean end-of-file.
    ///
    /// If the entry cannot be read the file is rewound to where it started, so the position is
    /// never left in the middle of an entry.
    fn read_entry(&mut self) -> Result<Option<T>, StoreError> {
        if let Some(start) = self.start_pos.take() {
            self.db_file.seek(io::SeekFrom::Start(start))?;
        }
        let pos_before_read = self.db_file.stream_position()?;

        // no bytes left: this is the end of the file, not a torn entry
        if self.db_file.fill_buf()?.is_empty() {
            return Ok(None);
        }

        let entry = self.read_frame();
        if entry.is_err() {
            self.db_file.seek(io::SeekFrom::Start(pos_before_read))?;
        }
        entry.map(Some)
    }

    /// Reads one length-prefixed frame and decodes it.
    ///
    /// A frame that ends early (end-of-file) or does not decode is [`StoreError::Decode`]. Any
    /// other failure to read is reported as [`StoreError::Io`].
    fn read_frame(&mut self) -> Result<T, StoreError> {
        let len = self.read_len_prefix()?;

        // `take` + `read_to_end` only allocates what the file actually holds, so a corrupt, huge
        // length prefix cannot trigger a huge allocation.
        let mut payload = Vec::new();
        (&mut self.db_file).take(len).read_to_end(&mut payload)?;
        if payload.len() as u64 != len {
            return Err(StoreError::Decode(
                postcard::Error::DeserializeUnexpectedEnd,
            ));
        }

        // The length prefix is authoritative, so bytes left over after decoding are corruption.
        match postcard::take_from_bytes(&payload) {
            Ok((entry, [])) => Ok(entry),
            Ok(_) => Err(StoreError::Decode(postcard::Error::SerdeDeCustom)),
            Err(e) => Err(StoreError::Decode(e)),
        }
    }

    /// Reads the `postcard` varint `u64` that holds the length of the frame's payload.
    fn read_len_prefix(&mut self) -> Result<u64, StoreError> {
        // a `u64` varint is at most 10 bytes; the high bit of a byte flags that another follows
        let mut buf = [0_u8; 10];
        for i in 0..buf.len() {
            if self.db_file.read(&mut buf[i..=i])? == 0 {
                return Err(StoreError::Decode(
                    postcard::Error::DeserializeUnexpectedEnd,
                ));
            }
            if buf[i] & 0x80 == 0 {
                return postcard::from_bytes(&buf[..=i]).map_err(StoreError::Decode);
            }
        }
        Err(StoreError::Decode(postcard::Error::DeserializeBadVarint))
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
