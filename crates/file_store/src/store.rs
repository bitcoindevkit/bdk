use crate::{bincode_options, EntryIter, FileError, IterError};
use bdk_chain::Merge;
use bincode::Options;
use std::{
    fmt::{self, Debug},
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    marker::PhantomData,
    path::Path,
};

/// Persists an append-only list of changesets (`C`) to a single file.
#[derive(Debug)]
pub struct Store<C>
where
    C: Sync + Send,
{
    magic_len: usize,
    db_file: File,
    marker: PhantomData<C>,
}

impl<C> Store<C>
where
    C: Merge
        + serde::Serialize
        + serde::de::DeserializeOwned
        + core::marker::Send
        + core::marker::Sync,
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
            .truncate(true)
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
                expected: magic.to_vec(),
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
    /// This function returns the aggregate changeset, or `None` if nothing was persisted.
    /// If reading or deserializing any of the entries fails, an error is returned that
    /// consists of all those it was able to read.
    ///
    /// You should usually check the error. In many applications, it may make sense to do a full
    /// wallet scan with a stop-gap after getting an error, since it is likely that one of the
    /// changesets was unable to read changes of the derivation indices of a keychain.
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
                Some(changeset) => changeset.merge(next_changeset),
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
                bincode::ErrorKind::Io(error) => error,
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

impl<C: fmt::Debug> std::error::Error for AggregateChangesetsError<C> {}

#[cfg(test)]
mod test {
    use super::*;

    use bincode::DefaultOptions;
    use std::{
        collections::BTreeSet,
        io::{Read, Write},
        vec::Vec,
    };
    use tempfile::NamedTempFile;

    const TEST_MAGIC_BYTES_LEN: usize = 12;
    const TEST_MAGIC_BYTES: [u8; TEST_MAGIC_BYTES_LEN] =
        [98, 100, 107, 102, 115, 49, 49, 49, 49, 49, 49, 49];

    type TestChangeSet = BTreeSet<String>;

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
        let changeset = BTreeSet::from(["hello".to_string(), "world".to_string()]);

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

        let changeset = TestChangeSet::from(["one".into(), "two".into(), "three!".into()]);

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

    #[test]
    fn last_write_is_short() {
        let temp_dir = tempfile::tempdir().unwrap();

        let changesets = [
            TestChangeSet::from(["1".into()]),
            TestChangeSet::from(["2".into(), "3".into()]),
            TestChangeSet::from(["4".into(), "5".into(), "6".into()]),
        ];
        let last_changeset = TestChangeSet::from(["7".into(), "8".into(), "9".into()]);
        let last_changeset_bytes = bincode_options().serialize(&last_changeset).unwrap();

        for short_write_len in 1..last_changeset_bytes.len() - 1 {
            let file_path = temp_dir.path().join(format!("{}.dat", short_write_len));

            // simulate creating a file, writing data where the last write is incomplete
            {
                let mut db =
                    Store::<TestChangeSet>::create_new(&TEST_MAGIC_BYTES, &file_path).unwrap();
                for changeset in &changesets {
                    db.append_changeset(changeset).unwrap();
                }
                // this is the incomplete write
                db.db_file
                    .write_all(&last_changeset_bytes[..short_write_len])
                    .unwrap();
            }

            // load file again and aggregate changesets
            // write the last changeset again (this time it succeeds)
            {
                let mut db = Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, &file_path).unwrap();
                let err = db
                    .aggregate_changesets()
                    .expect_err("should return error as last read is short");
                assert_eq!(
                    err.changeset,
                    changesets.iter().cloned().reduce(|mut acc, cs| {
                        Merge::merge(&mut acc, cs);
                        acc
                    }),
                    "should recover all changesets that are written in full",
                );
                db.db_file.write_all(&last_changeset_bytes).unwrap();
            }

            // load file again - this time we should successfully aggregate all changesets
            {
                let mut db = Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, &file_path).unwrap();
                let aggregated_changesets = db
                    .aggregate_changesets()
                    .expect("aggregating all changesets should succeed");
                assert_eq!(
                    aggregated_changesets,
                    changesets
                        .iter()
                        .cloned()
                        .chain(core::iter::once(last_changeset.clone()))
                        .reduce(|mut acc, cs| {
                            Merge::merge(&mut acc, cs);
                            acc
                        }),
                    "should recover all changesets",
                );
            }
        }
    }

    #[test]
    fn write_after_short_read() {
        let temp_dir = tempfile::tempdir().unwrap();

        let changesets = (0..20)
            .map(|n| TestChangeSet::from([format!("{}", n)]))
            .collect::<Vec<_>>();
        let last_changeset = TestChangeSet::from(["last".into()]);

        for read_count in 0..changesets.len() {
            let file_path = temp_dir.path().join(format!("{}.dat", read_count));

            // First, we create the file with all the changesets!
            let mut db = Store::<TestChangeSet>::create_new(&TEST_MAGIC_BYTES, &file_path).unwrap();
            for changeset in &changesets {
                db.append_changeset(changeset).unwrap();
            }
            drop(db);

            // We re-open the file and read `read_count` number of changesets.
            let mut db = Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, &file_path).unwrap();
            let mut exp_aggregation = db
                .iter_changesets()
                .take(read_count)
                .map(|r| r.expect("must read valid changeset"))
                .fold(TestChangeSet::default(), |mut acc, v| {
                    Merge::merge(&mut acc, v);
                    acc
                });
            // We write after a short read.
            db.append_changeset(&last_changeset)
                .expect("last write must succeed");
            Merge::merge(&mut exp_aggregation, last_changeset.clone());
            drop(db);

            // We open the file again and check whether aggregate changeset is expected.
            let aggregation = Store::<TestChangeSet>::open(&TEST_MAGIC_BYTES, &file_path)
                .unwrap()
                .aggregate_changesets()
                .expect("must aggregate changesets")
                .unwrap_or_default();
            assert_eq!(aggregation, exp_aggregation);
        }
    }
}
