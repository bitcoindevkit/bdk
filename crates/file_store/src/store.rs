use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    marker::PhantomData,
    path::Path,
};

use bdk_chain::Append;
use bincode::Options;

use crate::{bincode_options, EntryIter, FileError, IterError};

/// Persists an append-only list of changesets (`C`) to a single file.
///
/// The changesets are the results of altering a tracker implementation (`T`).
#[derive(Debug)]
pub struct Store<T, C> {
    magic: &'static [u8],
    db_file: File,
    marker: PhantomData<(T, C)>,
}

impl<T, C> Store<T, C>
where
    C: Default + Append + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Creates a new store from a [`File`].
    ///
    /// The file must have been opened with read and write permissions.
    ///
    /// [`File`]: std::fs::File
    pub fn new(magic: &'static [u8], mut db_file: File) -> Result<Self, FileError> {
        db_file.rewind()?;

        let mut magic_buf = Vec::from_iter((0..).take(magic.len()));
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
    pub fn new_from_path<P>(magic: &'static [u8], db_path: P) -> Result<Self, FileError>
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
    pub fn iter_changesets(&mut self) -> Result<EntryIter<'_, C>, io::Error> {
        self.db_file
            .seek(io::SeekFrom::Start(self.magic.len() as _))?;

        Ok(EntryIter::new(&mut self.db_file))
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
            let iter_changeset = self.iter_changesets()?;
            for next_changeset in iter_changeset {
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
    ///
    /// **WARNING**: This method does not detect whether the changeset is empty or not, and will
    /// append an empty changeset to the file (not catastrophic, just a waste of space).
    pub fn append_changeset(&mut self, changeset: &C) -> Result<(), io::Error> {
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

    #[derive(
        Debug,
        Clone,
        Copy,
        PartialOrd,
        Ord,
        PartialEq,
        Eq,
        Hash,
        serde::Serialize,
        serde::Deserialize,
    )]
    enum TestKeychain {
        External,
        Internal,
    }

    impl core::fmt::Display for TestKeychain {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::External => write!(f, "external"),
                Self::Internal => write!(f, "internal"),
            }
        }
    }

    #[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
    struct TestChangeSet {
        pub changes: Vec<String>,
    }

    impl Append for TestChangeSet {
        fn append(&mut self, mut other: Self) {
            self.changes.append(&mut other.changes)
        }
    }

    #[derive(Debug)]
    struct TestTracker;

    #[test]
    fn new_fails_if_file_is_too_short() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&TEST_MAGIC_BYTES[..TEST_MAGIC_BYTES_LEN - 1])
            .expect("should write");

        match Store::<TestTracker, TestChangeSet>::new(&TEST_MAGIC_BYTES, file.reopen().unwrap()) {
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

        match Store::<TestTracker, TestChangeSet>::new(&TEST_MAGIC_BYTES, file.reopen().unwrap()) {
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

        let changeset = TestChangeSet {
            changes: vec!["one".into(), "two".into(), "three!".into()],
        };

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&data).expect("should write");

        let mut store =
            Store::<TestTracker, TestChangeSet>::new(&TEST_MAGIC_BYTES, file.reopen().unwrap())
                .expect("should open");
        match store.iter_changesets().expect("seek should succeed").next() {
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
