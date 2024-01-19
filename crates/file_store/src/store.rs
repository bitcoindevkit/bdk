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
pub struct Store<C> {
    magic_len: usize,
    db_file: File,
    marker: PhantomData<C>,
}

impl<C> PersistBackend<C> for Store<C>
where
    C: Append + serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = std::io::Error;

    type LoadError = IterError;

    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_from_persistence(&mut self) -> Result<Option<C>, Self::LoadError> {
        self.aggregate_changesets().map_err(|e| e.iter_error)
    }
}

impl<C> Store<C>
where
    C: Append + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Create a new [`Store`] file in write-only mode; error if the file exists.
    ///
    /// `magic` is the prefixed bytes to write to the new file. This will be checked when opening
    /// the `Store` in the future with [`open`].
    ///
    /// [`open`]: Store::open
    pub fn create_new<P>(magic: &[u8], file_path: P) -> Result<Self, FileError>
    where
        P: AsRef<Path>,
    {
        if file_path.as_ref().exists() {
            // `io::Error` is used instead of a variant on `FileError` because there is already a
            // nightly-only `File::create_new` method
            return Err(FileError::Io(io::Error::new(
                io::ErrorKind::Other,
                "file already exists",
            )));
        }
        let mut f = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(file_path)?;
        f.write_all(magic)?;
        Ok(Self {
            magic_len: magic.len(),
            db_file: f,
            marker: Default::default(),
        })
    }

    /// Open an existing [`Store`].
    ///
    /// Use [`create_new`] to create a new `Store`.
    ///
    /// # Errors
    ///
    /// If the prefixed bytes of the opened file does not match the provided `magic`, the
    /// [`FileError::InvalidMagicBytes`] error variant will be returned.
    ///
    /// [`create_new`]: Store::create_new
    pub fn open<P>(magic: &[u8], file_path: P) -> Result<Self, FileError>
    where
        P: AsRef<Path>,
    {
        let mut f = OpenOptions::new().read(true).write(true).open(file_path)?;

        let mut magic_buf = vec![0_u8; magic.len()];
        f.read_exact(&mut magic_buf)?;
        if magic_buf != magic {
            return Err(FileError::InvalidMagicBytes {
                got: magic_buf,
                expected: magic,
            });
        }

        Ok(Self {
            magic_len: magic.len(),
            db_file: f,
            marker: Default::default(),
        })
    }

    /// Attempt to open existing [`Store`] file; create it if the file is non-existent.
    ///
    /// Internally, this calls either [`open`] or [`create_new`].
    ///
    /// [`open`]: Store::open
    /// [`create_new`]: Store::create_new
    pub fn open_or_create_new<P>(magic: &[u8], file_path: P) -> Result<Self, FileError>
    where
        P: AsRef<Path>,
    {
        if file_path.as_ref().exists() {
            Self::open(magic, file_path)
        } else {
            Self::create_new(magic, file_path)
        }
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
        EntryIter::new(self.magic_len as u64, &mut self.db_file)
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
    pub fn aggregate_changesets(&mut self) -> Result<Option<C>, AggregateChangesetsError<C>> {
        let mut changeset = Option::<C>::None;
        for next_changeset in self.iter_changesets() {
            let next_changeset = match next_changeset {
                Ok(next_changeset) => next_changeset,
                Err(iter_error) => {
                    return Err(AggregateChangesetsError {
                        changeset,
                        iter_error,
                    })
                }
            };
            match &mut changeset {
                Some(changeset) => changeset.append(next_changeset),
                changeset => *changeset = Some(next_changeset),
            }
        }
        Ok(changeset)
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

/// Error type for [`Store::aggregate_changesets`].
#[derive(Debug)]
pub struct AggregateChangesetsError<C> {
    /// The partially-aggregated changeset.
    pub changeset: Option<C>,

    /// The error returned by [`EntryIter`].
    pub iter_error: IterError,
}

impl<C> std::fmt::Display for AggregateChangesetsError<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.iter_error, f)
    }
}

impl<C: std::fmt::Debug> std::error::Error for AggregateChangesetsError<C> {}

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

    /// Check behavior of [`Store::create_new`] and [`Store::open`].
    #[test]
    fn construct_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let _ = Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, &file_path)
            .expect_err("must not open as file does not exist yet");
        let _ = Store::<TestChangeSet>::create_new(&TEST_MAGIC_BYTES, &file_path)
            .expect("must create file");
        // cannot create new as file already exists
        let _ = Store::<TestChangeSet>::create_new(&TEST_MAGIC_BYTES, &file_path)
            .expect_err("must fail as file already exists now");
        let _ = Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, &file_path)
            .expect("must open as file exists now");
    }

    #[test]
    fn open_or_create_new() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let changeset = vec!["hello".to_string(), "world".to_string()];

        {
            let mut db = Store::<TestChangeSet>::open_or_create_new(&TEST_MAGIC_BYTES, &file_path)
                .expect("must create");
            assert!(file_path.exists());
            db.append_changeset(&changeset).expect("must succeed");
        }

        {
            let mut db = Store::<TestChangeSet>::open_or_create_new(&TEST_MAGIC_BYTES, &file_path)
                .expect("must recover");
            let recovered_changeset = db.aggregate_changesets().expect("must succeed");
            assert_eq!(recovered_changeset, Some(changeset));
        }
    }

    #[test]
    fn new_fails_if_file_is_too_short() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&TEST_MAGIC_BYTES[..TEST_MAGIC_BYTES_LEN - 1])
            .expect("should write");

        match Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, file.path()) {
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

        match Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, file.path()) {
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

        let mut store =
            Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, file.path()).expect("should open");
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
