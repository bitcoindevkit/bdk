use std::{
    fmt::Debug,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    marker::PhantomData,
    path::Path,
};

use bdk_chain::{Append, PersistBackend};
use bincode::Options;

use crate::{bincode_options, EntryIter, FileError, IterError};

/// Persists an append-only list of changesets (`C`) to a single file.
///
/// The changesets are the results of altering a tracker implementation (`T`).
#[derive(Debug)]
pub struct Store<'a, C> {
    magic: &'a [u8],
    db_file: File,
    marker: PhantomData<C>,
}

impl<'a, C> PersistBackend<C> for Store<'a, C>
where
    C: Default + Append + serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = std::io::Error;

    type LoadError = IterError;

    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_from_persistence(&mut self) -> Result<C, Self::LoadError> {
        let (changeset, result) = self.aggregate_changesets();
        result.map(|_| changeset)
    }
}

impl<'a, C> Store<'a, C>
where
    C: Default + Append + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Creates a new store from a [`File`].
    ///
    /// The file must have been opened with read and write permissions.
    ///
    /// `magic` is the expected prefixed bytes of the file. If this does not match, an error will be
    /// returned.
    ///
    /// [`File`]: std::fs::File
    pub fn new(magic: &'a [u8], mut db_file: File) -> Result<Self, FileError> {
        db_file.rewind()?;

        let mut magic_buf = vec![0_u8; magic.len()];
        db_file.read_exact(magic_buf.as_mut())?;

        if magic_buf != magic {
            return Err(FileError::InvalidMagicBytes {
                got: magic_buf,
                expected: magic,
            });
        }

        Ok(Self {
            magic,
            db_file,
            marker: Default::default(),
        })
    }

    /// Creates or loads a store from `db_path`.
    ///
    /// If no file exists there, it will be created.
    ///
    /// Refer to [`new`] for documentation on the `magic` input.
    ///
    /// [`new`]: Self::new
    pub fn new_from_path<P>(magic: &'a [u8], db_path: P) -> Result<Self, FileError>
    where
        P: AsRef<Path>,
    {
        let already_exists = db_path.as_ref().exists();

        let mut db_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(db_path)?;

        if !already_exists {
            db_file.write_all(magic)?;
        }

        Self::new(magic, db_file)
    }

    /// Iterates over the stored changeset from first to last, changing the seek position at each
    /// iteration.
    ///
    /// The iterator may fail to read an entry and therefore return an error. However, the first time
    /// it returns an error will be the last. After doing so, the iterator will always yield `None`.
    ///
    /// **WARNING**: This method changes the write position in the underlying file. You should
    /// always iterate over all entries until `None` is returned if you want your next write to go
    /// at the end; otherwise, you will write over existing entries.
    pub fn iter_changesets(&mut self) -> EntryIter<C> {
        EntryIter::new(self.magic.len() as u64, &mut self.db_file)
    }

    /// Loads all the changesets that have been stored as one giant changeset.
    ///
    /// This function returns a tuple of the aggregate changeset and a result that indicates
    /// whether an error occurred while reading or deserializing one of the entries. If so the
    /// changeset will consist of all of those it was able to read.
    ///
    /// You should usually check the error. In many applications, it may make sense to do a full
    /// wallet scan with a stop-gap after getting an error, since it is likely that one of the
    /// changesets it was unable to read changed the derivation indices of the tracker.
    ///
    /// **WARNING**: This method changes the write position of the underlying file. The next
    /// changeset will be written over the erroring entry (or the end of the file if none existed).
    pub fn aggregate_changesets(&mut self) -> (C, Result<(), IterError>) {
        let mut changeset = C::default();
        let result = (|| {
            for next_changeset in self.iter_changesets() {
                changeset.append(next_changeset?);
            }
            Ok(())
        })();

        (changeset, result)
    }

    /// Append a new changeset to the file and truncate the file to the end of the appended
    /// changeset.
    ///
    /// The truncation is to avoid the possibility of having a valid but inconsistent changeset
    /// directly after the appended changeset.
    pub fn append_changeset(&mut self, changeset: &C) -> Result<(), io::Error> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        bincode_options()
            .serialize_into(&mut self.db_file, changeset)
            .map_err(|e| match *e {
                bincode::ErrorKind::Io(inner) => inner,
                unexpected_err => panic!("unexpected bincode error: {}", unexpected_err),
            })?;

        // truncate file after this changeset addition
        // if this is not done, data after this changeset may represent valid changesets, however
        // applying those changesets on top of this one may result in an inconsistent state
        let pos = self.db_file.stream_position()?;
        self.db_file.set_len(pos)?;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use bincode::DefaultOptions;
    use std::{
        io::{Read, Write},
        vec::Vec,
    };
    use tempfile::NamedTempFile;

    const TEST_MAGIC_BYTES_LEN: usize = 12;
    const TEST_MAGIC_BYTES: [u8; TEST_MAGIC_BYTES_LEN] =
        [98, 100, 107, 102, 115, 49, 49, 49, 49, 49, 49, 49];

    type TestChangeSet = Vec<String>;

    #[derive(Debug)]
    struct TestTracker;

    #[test]
    fn new_fails_if_file_is_too_short() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&TEST_MAGIC_BYTES[..TEST_MAGIC_BYTES_LEN - 1])
            .expect("should write");

        match Store::<TestChangeSet>::new(&TEST_MAGIC_BYTES, file.reopen().unwrap()) {
            Err(FileError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            unexpected => panic!("unexpected result: {:?}", unexpected),
        };
    }

    #[test]
    fn new_fails_if_magic_bytes_are_invalid() {
        let invalid_magic_bytes = "ldkfs0000000";

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(invalid_magic_bytes.as_bytes())
            .expect("should write");

        match Store::<TestChangeSet>::new(&TEST_MAGIC_BYTES, file.reopen().unwrap()) {
            Err(FileError::InvalidMagicBytes { got, .. }) => {
                assert_eq!(got, invalid_magic_bytes.as_bytes())
            }
            unexpected => panic!("unexpected result: {:?}", unexpected),
        };
    }

    #[test]
    fn append_changeset_truncates_invalid_bytes() {
        // initial data to write to file (magic bytes + invalid data)
        let mut data = [255_u8; 2000];
        data[..TEST_MAGIC_BYTES_LEN].copy_from_slice(&TEST_MAGIC_BYTES);

        let changeset = vec!["one".into(), "two".into(), "three!".into()];

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&data).expect("should write");

        let mut store = Store::<TestChangeSet>::new(&TEST_MAGIC_BYTES, file.reopen().unwrap())
            .expect("should open");
        match store.iter_changesets().next() {
            Some(Err(IterError::Bincode(_))) => {}
            unexpected_res => panic!("unexpected result: {:?}", unexpected_res),
        }

        store.append_changeset(&changeset).expect("should append");

        drop(store);

        let got_bytes = {
            let mut buf = Vec::new();
            file.reopen()
                .unwrap()
                .read_to_end(&mut buf)
                .expect("should read");
            buf
        };

        let expected_bytes = {
            let mut buf = TEST_MAGIC_BYTES.to_vec();
            DefaultOptions::new()
                .with_varint_encoding()
                .serialize_into(&mut buf, &changeset)
                .expect("should encode");
            buf
        };

        assert_eq!(got_bytes, expected_bytes);
    }
}
