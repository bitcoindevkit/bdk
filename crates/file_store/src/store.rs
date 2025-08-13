use crate::{bincode_options, EntryIter, StoreError};
use bdk_core::Merge;
use bincode::Options;
use std::{
    fmt::{self, Debug},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    marker::PhantomData,
    path::Path,
};

/// Persists an append-only list of changesets (`C`) to a single file.
///
/// > ⚠ This is a development/testing database. It does not natively support backwards compatible
/// > BDK version upgrades so should not be used in production.
#[derive(Debug)]
pub struct Store<C> {
    magic_len: usize,
    db_file: File,
    marker: PhantomData<C>,
}

impl<C> Store<C>
where
    C: Merge + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Create a new [`Store`] file in write-only mode; error if the file exists.
    ///
    /// `magic` is the prefixed bytes to write to the new file. This will be checked when loading
    /// the [`Store`] in the future with [`load`].
    ///
    /// [`load`]: Store::load
    pub fn create<P>(magic: &[u8], file_path: P) -> Result<Self, StoreError>
    where
        P: AsRef<Path>,
    {
        let mut f = OpenOptions::new()
            .create_new(true)
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

    /// Load an existing [`Store`].
    ///
    /// Use [`create`] to create a new [`Store`].
    ///
    /// # Errors
    ///
    /// If the prefixed bytes of the loaded file do not match the provided `magic`, a
    /// [`StoreErrorWithDump`] will be returned with the [`StoreError::InvalidMagicBytes`] error
    /// variant in its error field and changeset field set to [`Option::None`]
    ///
    /// If there exist changesets in the file, [`load`] will try to aggregate them in
    /// a single changeset to verify their integrity. If aggregation fails
    /// [`StoreErrorWithDump`] will be returned with the [`StoreError::Bincode`] error variant in
    /// its error field and the aggregated changeset so far in the changeset field.
    ///
    /// To get a new working file store from this error use [`Store::create`] and [`Store::append`]
    /// to add the aggregated changeset obtained from [`StoreErrorWithDump`].
    ///
    /// To analyze the causes of the problem in the original database do not recreate the [`Store`]
    /// using the same file path. Not changing the file path will overwrite previous file without
    /// being able to recover its original data.
    ///
    /// # Examples
    /// ```
    /// use bdk_file_store::{Store, StoreErrorWithDump};
    /// # use std::fs::OpenOptions;
    /// # use bdk_core::Merge;
    /// # use std::collections::BTreeSet;
    /// # use std::io;
    /// # use std::io::SeekFrom;
    /// # use std::io::{Seek, Write};
    /// #
    /// # fn main() -> io::Result<()> {
    /// # const MAGIC_BYTES_LEN: usize = 12;
    /// # const MAGIC_BYTES: [u8; MAGIC_BYTES_LEN] =
    /// #     [98, 100, 107, 102, 115, 49, 49, 49, 49, 49, 49, 49];
    /// #
    /// # type TestChangeSet = BTreeSet<String>;
    /// #
    /// # let temp_dir = tempfile::tempdir()?;
    /// # let file_path = temp_dir.path().join("db_file");
    /// # let mut store = Store::<TestChangeSet>::create(&MAGIC_BYTES, &file_path).unwrap();
    /// # let changesets = [
    /// #     TestChangeSet::from(["1".into()]),
    /// #     TestChangeSet::from(["2".into(), "3".into()]),
    /// #     TestChangeSet::from(["4".into(), "5".into(), "6".into()]),
    /// # ];
    /// #
    /// # for changeset in &changesets[..] {
    /// #     store.append(changeset)?;
    /// # }
    /// #
    /// # drop(store);
    /// #
    /// # // Simulate the file is broken
    /// # let mut data = [255_u8; 2000];
    /// # data[..MAGIC_BYTES_LEN].copy_from_slice(&MAGIC_BYTES);
    /// # let mut file = OpenOptions::new().append(true).open(file_path.clone())?;
    /// # let new_len = file.seek(SeekFrom::End(-2))?;
    /// # file.set_len(new_len)?;
    ///
    /// let (mut new_store, _aggregate_changeset) =
    ///     match Store::<TestChangeSet>::load(&MAGIC_BYTES, &file_path) {
    /// #   Ok(_) => panic!("should have errored"),
    ///         Ok((store, changeset)) => (store, changeset),
    ///         Err(StoreErrorWithDump { changeset, .. }) => {
    ///             let new_file_path = file_path.with_extension("backup");
    ///             let mut new_store =
    ///                 Store::create(&MAGIC_BYTES, &new_file_path).expect("must create new file");
    ///             if let Some(aggregated_changeset) = changeset {
    ///                 new_store.append(aggregated_changeset.as_ref())?;
    ///             }
    ///             // The following will overwrite the original file. You will loose the corrupted
    ///             // portion of the original file forever.
    ///             drop(new_store);
    ///             std::fs::rename(&new_file_path, &file_path)?;
    ///             Store::load(&MAGIC_BYTES, &file_path).expect("must load new file")
    ///         }
    ///     };
    /// #
    /// # assert_eq!(
    /// #     new_store.dump().expect("should dump changeset: {1, 2, 3} "),
    /// #     changesets[..2].iter().cloned().reduce(|mut acc, cs| {
    /// #         Merge::merge(&mut acc, cs);
    /// #         acc
    /// #     }),
    /// #     "should recover all changesets",
    /// # );
    /// #
    /// # Ok(())
    /// # }
    /// ```
    /// [`create`]: Store::create
    /// [`load`]: Store::load
    pub fn load<P>(magic: &[u8], file_path: P) -> Result<(Self, Option<C>), StoreErrorWithDump<C>>
    where
        P: AsRef<Path>,
    {
        let mut f = OpenOptions::new().read(true).write(true).open(file_path)?;

        let mut magic_buf = vec![0_u8; magic.len()];
        f.read_exact(&mut magic_buf)?;
        if magic_buf != magic {
            return Err(StoreErrorWithDump {
                changeset: Option::<Box<C>>::None,
                error: StoreError::InvalidMagicBytes {
                    got: magic_buf,
                    expected: magic.to_vec(),
                },
            });
        }

        let mut store = Self {
            magic_len: magic.len(),
            db_file: f,
            marker: Default::default(),
        };

        // Get aggregated changeset
        let aggregated_changeset = store.dump()?;

        Ok((store, aggregated_changeset))
    }

    /// Dump the aggregate of all changesets in [`Store`].
    ///
    /// # Errors
    ///
    /// If there exist changesets in the file, [`dump`] will try to aggregate them in a single
    /// changeset. If aggregation fails [`StoreErrorWithDump`] will be returned with the
    /// [`StoreError::Bincode`] error variant in its error field and the aggregated changeset so
    /// far in the changeset field.
    ///
    /// [`dump`]: Store::dump
    pub fn dump(&mut self) -> Result<Option<C>, StoreErrorWithDump<C>> {
        EntryIter::new(self.magic_len as u64, &mut self.db_file).try_fold(
            Option::<C>::None,
            |mut aggregated_changeset: Option<C>, next_changeset| match next_changeset {
                Ok(next_changeset) => {
                    match &mut aggregated_changeset {
                        Some(aggregated_changeset) => aggregated_changeset.merge(next_changeset),
                        aggregated_changeset => *aggregated_changeset = Some(next_changeset),
                    }
                    Ok(aggregated_changeset)
                }
                Err(iter_error) => Err(StoreErrorWithDump {
                    changeset: aggregated_changeset.map(Box::new),
                    error: iter_error,
                }),
            },
        )
    }

    /// Attempt to load existing [`Store`] file; create it if the file does not exist.
    ///
    /// Internally, this calls either [`load`] or [`create`].
    ///
    /// [`load`]: Store::load
    /// [`create`]: Store::create
    pub fn load_or_create<P>(
        magic: &[u8],
        file_path: P,
    ) -> Result<(Self, Option<C>), StoreErrorWithDump<C>>
    where
        P: AsRef<Path>,
    {
        if file_path.as_ref().exists() {
            Self::load(magic, file_path)
        } else {
            Self::create(magic, file_path)
                .map(|store| (store, Option::<C>::None))
                .map_err(|err: StoreError| StoreErrorWithDump {
                    changeset: Option::<Box<C>>::None,
                    error: err,
                })
        }
    }

    /// Append a new changeset to the file. Does nothing if the changeset is empty. Truncation is
    /// not needed because file pointer is always moved to the end of the last decodable data from
    /// beginning to end.
    ///
    /// If multiple garbage writes are produced on the file, the next load will only retrieve the
    /// first chunk of valid changesets.
    ///
    /// If garbage data is written and then valid changesets, the next load will still only
    /// retrieve the first chunk of valid changesets. The recovery of those valid changesets after
    /// the garbage data is responsibility of the user.
    pub fn append(&mut self, changeset: &C) -> Result<(), io::Error> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        bincode_options()
            .serialize_into(&mut self.db_file, changeset)
            .map_err(|e| match *e {
                bincode::ErrorKind::Io(error) => error,
                unexpected_err => panic!("unexpected bincode error: {unexpected_err}"),
            })?;

        Ok(())
    }
}

/// Error type for [`Store::dump`].
#[derive(Debug)]
pub struct StoreErrorWithDump<C> {
    /// The partially-aggregated changeset.
    pub changeset: Option<Box<C>>,

    /// The [`StoreError`]
    pub error: StoreError,
}

impl<C> From<io::Error> for StoreErrorWithDump<C> {
    fn from(value: io::Error) -> Self {
        Self {
            changeset: Option::<Box<C>>::None,
            error: StoreError::Io(value),
        }
    }
}

impl<C> std::fmt::Display for StoreErrorWithDump<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}

impl<C: fmt::Debug> std::error::Error for StoreErrorWithDump<C> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use super::*;

    use std::{
        collections::BTreeSet,
        fs,
        io::{Seek, Write},
    };

    const TEST_MAGIC_BYTES_LEN: usize = 12;
    const TEST_MAGIC_BYTES: [u8; TEST_MAGIC_BYTES_LEN] =
        [98, 100, 107, 102, 115, 49, 49, 49, 49, 49, 49, 49];

    type TestChangeSet = BTreeSet<String>;

    /// Check behavior of [`Store::create`] and [`Store::load`].
    #[test]
    fn construct_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let _ = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
            .expect_err("must not open as file does not exist yet");
        let _ = Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path)
            .expect("must create file");
        // cannot create new as file already exists
        let _ = Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path)
            .expect_err("must fail as file already exists now");
        let _ = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
            .expect("must open as file exists now");
    }

    #[test]
    fn load_fails_if_file_is_too_short() {
        let tempdir = tempfile::tempdir().unwrap();
        let file_path = tempdir.path().join("db_file");
        fs::write(&file_path, &TEST_MAGIC_BYTES[..TEST_MAGIC_BYTES_LEN - 1]).expect("should write");

        match Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path) {
            Err(StoreErrorWithDump {
                error: StoreError::Io(e),
                ..
            }) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            unexpected => panic!("unexpected result: {unexpected:?}"),
        };
    }

    #[test]
    fn load_fails_if_magic_bytes_are_invalid() {
        let invalid_magic_bytes = "ldkfs0000000";

        let tempdir = tempfile::tempdir().unwrap();
        let file_path = tempdir.path().join("db_file");
        fs::write(&file_path, invalid_magic_bytes.as_bytes()).expect("should write");

        match Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path) {
            Err(StoreErrorWithDump {
                error: StoreError::InvalidMagicBytes { got, .. },
                ..
            }) => {
                assert_eq!(got, invalid_magic_bytes.as_bytes())
            }
            unexpected => panic!("unexpected result: {unexpected:?}"),
        };
    }

    #[test]
    fn load_fails_if_undecodable_bytes() {
        // initial data to write to file (magic bytes + invalid data)
        let mut data = [255_u8; 2000];
        data[..TEST_MAGIC_BYTES_LEN].copy_from_slice(&TEST_MAGIC_BYTES);

        let test_changesets = TestChangeSet::from(["one".into(), "two".into(), "three!".into()]);

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let mut store =
            Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).expect("should create");
        store.append(&test_changesets).expect("should append");

        // Write garbage to file
        store.db_file.write_all(&data).expect("should write");

        drop(store);

        match Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, file_path) {
            Err(StoreErrorWithDump {
                changeset,
                error: StoreError::Bincode(_),
            }) => {
                assert_eq!(changeset, Some(Box::new(test_changesets)))
            }
            unexpected_res => panic!("unexpected result: {unexpected_res:?}"),
        }
    }

    #[test]
    fn dump_fails_if_undecodable_bytes() {
        // initial data to write to file (magic bytes + invalid data)
        let mut data = [255_u8; 2000];
        data[..TEST_MAGIC_BYTES_LEN].copy_from_slice(&TEST_MAGIC_BYTES);

        let test_changesets = TestChangeSet::from(["one".into(), "two".into(), "three!".into()]);

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let mut store =
            Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, file_path).expect("should create");
        store.append(&test_changesets).expect("should append");

        // Write garbage to file
        store.db_file.write_all(&data).expect("should write");

        match store.dump() {
            Err(StoreErrorWithDump {
                changeset,
                error: StoreError::Bincode(_),
            }) => {
                assert_eq!(changeset, Some(Box::new(test_changesets)))
            }
            unexpected_res => panic!("unexpected result: {unexpected_res:?}"),
        }
    }

    #[test]
    fn append() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");

        let not_empty_changeset = BTreeSet::from(["hello".to_string(), "world".to_string()]);

        let mut store =
            Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, file_path).expect("must create");

        store
            .append(&not_empty_changeset)
            .expect("must append changeset");
        let aggregated_changeset = store
            .dump()
            .expect("should aggregate")
            .expect("should not be empty");
        assert_eq!(not_empty_changeset, aggregated_changeset);
    }

    #[test]
    fn append_empty_changeset_does_nothing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");

        let empty_changeset = BTreeSet::new();

        let mut store =
            Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, file_path).expect("must create");

        store
            .append(&empty_changeset)
            .expect("must append changeset");
        let aggregated_changeset = store.dump().expect("should aggregate");
        assert_eq!(None, aggregated_changeset);
    }

    #[test]
    fn load_or_create() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let changeset = BTreeSet::from(["hello".to_string(), "world".to_string()]);

        {
            let (mut store, _) =
                Store::<TestChangeSet>::load_or_create(&TEST_MAGIC_BYTES, &file_path)
                    .expect("must create");
            assert!(file_path.exists());
            store.append(&changeset).expect("must succeed");
        }

        {
            let (_, recovered_changeset) =
                Store::<TestChangeSet>::load_or_create(&TEST_MAGIC_BYTES, &file_path)
                    .expect("must load");
            assert_eq!(recovered_changeset, Some(changeset));
        }
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
            let file_path = temp_dir.path().join(format!("{short_write_len}.dat"));

            // simulate creating a file, writing data where the last write is incomplete
            {
                let mut store =
                    Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).unwrap();
                for changeset in &changesets {
                    store.append(changeset).unwrap();
                }
                // this is the incomplete write
                store
                    .db_file
                    .write_all(&last_changeset_bytes[..short_write_len])
                    .unwrap();
            }

            // load file again and aggregate changesets
            // write the last changeset again (this time it succeeds)
            {
                let err = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
                    .expect_err("should fail to aggregate");
                assert_eq!(
                    err.changeset,
                    changesets
                        .iter()
                        .cloned()
                        .reduce(|mut acc, cs| {
                            Merge::merge(&mut acc, cs);
                            acc
                        })
                        .map(Box::new),
                    "should recover all changesets that are written in full",
                );
                // Remove file and start again
                fs::remove_file(&file_path).expect("should remove file");
                let mut store =
                    Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).unwrap();
                for changeset in &changesets {
                    store.append(changeset).unwrap();
                }
                // this is the complete write
                store
                    .db_file
                    .write_all(&last_changeset_bytes)
                    .expect("should write last changeset in full");
            }

            // load file again - this time we should successfully aggregate all changesets
            {
                let (_, aggregated_changeset) =
                    Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path).unwrap();
                assert_eq!(
                    aggregated_changeset,
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
    fn test_load_recovers_state_after_last_write() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let changeset1 = BTreeSet::from(["hello".to_string(), "world".to_string()]);
        let changeset2 = BTreeSet::from(["change after write".to_string()]);

        {
            // create new store
            let mut store =
                Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).expect("must create");

            // append first changeset to store
            store.append(&changeset1).expect("must succeed");
        }

        {
            // open store
            let (mut store, _) = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
                .expect("failed to load store");

            // now append the second changeset
            store.append(&changeset2).expect("must succeed");

            // Retrieve stored changesets from the database
            let stored_changesets = store
                .dump()
                .expect("must succeed")
                .expect("must be not empty");

            // expected changeset must be changeset2 + changeset1
            let mut expected_changeset = changeset2.clone();
            expected_changeset.extend(changeset1);

            // Assert that stored_changesets matches expected_changeset but not changeset2
            assert_eq!(stored_changesets, expected_changeset);
            assert_ne!(stored_changesets, changeset2);
        }

        // Open the store again to verify file pointer position at the end of the file
        let (mut store, _) = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
            .expect("should load correctly");

        // get the current position of file pointer just after loading store
        let current_pointer = store.db_file.stream_position().expect("must suceed");

        // end pointer for the loaded store
        let expected_pointer = store
            .db_file
            .seek(io::SeekFrom::End(0))
            .expect("must succeed");

        // current position matches EOF
        assert_eq!(current_pointer, expected_pointer);
    }
}
