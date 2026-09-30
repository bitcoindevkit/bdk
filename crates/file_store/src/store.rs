use crate::{EntryIter, StoreError};
use bdk_core::Merge;
use std::{
    fmt::{self, Debug},
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
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
    /// [`StoreErrorWithDump`] will be returned with the [`StoreError::Decode`] error variant in
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
    /// [`StoreError::Decode`] error variant in its error field and the aggregated changeset so
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

    /// Append a new changeset to the file. Does nothing if the changeset is empty.
    ///
    /// The changeset is always written at the current end of the file, so appending through a
    /// handle whose file position is stale (for example, because another handle has appended
    /// since this handle last read the file) will not overwrite existing changesets. If a write
    /// fails partway through, the partial frame is truncated before the error is returned, so a
    /// failed append leaves the file unchanged.
    ///
    /// Appending to a file that contains undecodable trailing data will not make that data
    /// readable; use the recovery procedure described in [`load`] instead.
    ///
    /// If multiple garbage writes are produced on the file, the next load will only retrieve the
    /// first chunk of valid changesets.
    ///
    /// If garbage data is written and then valid changesets, the next load will still only
    /// retrieve the first chunk of valid changesets. The recovery of those valid changesets after
    /// the garbage data is responsibility of the user.
    ///
    /// [`load`]: Store::load
    pub fn append(&mut self, changeset: &C) -> Result<(), io::Error> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        let bytes = postcard::to_allocvec(changeset).map_err(io::Error::other)?;
        let len_bytes = postcard::to_allocvec(&(bytes.len() as u64)).map_err(io::Error::other)?;

        // Always write at the current end of the file. This handle's cursor may be stale if
        // another handle has appended since we last read, and writing at a stale offset would
        // overwrite those changesets.
        let start = self.db_file.seek(io::SeekFrom::End(0))?;

        let result = self
            .db_file
            .write_all(&len_bytes)
            .and_then(|()| self.db_file.write_all(&bytes));
        if let Err(e) = result {
            // Roll back the partial frame so a failed append leaves no torn data behind.
            let _ = self.db_file.set_len(start);
            return Err(e);
        }

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

impl<C: fmt::Debug> core::error::Error for StoreErrorWithDump<C> {}

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
                error: StoreError::Decode(_),
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
                error: StoreError::Decode(_),
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
        let last_changeset_payload = postcard::to_allocvec(&last_changeset).unwrap();
        let mut last_changeset_bytes =
            postcard::to_allocvec(&(last_changeset_payload.len() as u64)).unwrap();
        last_changeset_bytes.extend_from_slice(&last_changeset_payload);

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

    #[test]
    fn load_does_not_panic_on_oversized_length_prefix() {
        // Build a file whose varint length prefix decodes to `u64::MAX`. Without a guard on the
        // length prefix, this would trigger an allocation of that many bytes, panicking/aborting
        // instead of returning a graceful error.
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&TEST_MAGIC_BYTES);
        let huge_len_encoded: Vec<u8> = postcard::to_allocvec(&u64::MAX).unwrap();
        bytes.extend_from_slice(&huge_len_encoded);

        std::fs::write(&file_path, &bytes).unwrap();

        let result = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path);
        assert!(
            result.is_err(),
            "load should fail gracefully on oversized length prefix"
        );
    }

    #[test]
    fn load_fails_on_frame_with_trailing_bytes() {
        // Craft a well-formed frame whose declared length is 2 bytes longer than the valid
        // payload it contains. The length prefix stays authoritative for framing, but the
        // payload itself doesn't consume its whole declared length, which must be treated as
        // corruption rather than silently ignored.
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");

        let changeset = TestChangeSet::from(["hello".to_string()]);
        let payload = postcard::to_allocvec(&changeset).unwrap();

        let mut bytes = TEST_MAGIC_BYTES.to_vec();
        bytes.extend_from_slice(&postcard::to_allocvec(&((payload.len() + 2) as u64)).unwrap());
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&[0xaa, 0xbb]);

        fs::write(&file_path, bytes).expect("should write crafted store");

        match Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path) {
            Err(StoreErrorWithDump {
                error: StoreError::Decode(_),
                ..
            }) => {}
            unexpected => panic!("unexpected result: {unexpected:?}"),
        }
    }

    #[test]
    fn load_fails_on_genuinely_undecodable_payload() {
        // Craft a frame with a correct length prefix but invalid payload (a string that claims 4
        // bytes which are not valid UTF-8).
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");

        let payload = vec![0x01, 0x04, 0xff, 0xff, 0xff, 0xff];
        let mut bytes = TEST_MAGIC_BYTES.to_vec();
        bytes.extend_from_slice(&postcard::to_allocvec(&(payload.len() as u64)).unwrap());
        bytes.extend_from_slice(&payload);

        fs::write(&file_path, bytes).expect("should write crafted store");

        match Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path) {
            Err(StoreErrorWithDump {
                error: StoreError::Decode(postcard::Error::DeserializeBadUtf8),
                ..
            }) => {}
            unexpected => panic!("unexpected result: {unexpected:?}"),
        }
    }

    // postcard encodes unit structs as a zero byte, i.e., 0x00 followed by no payload at all
    #[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
    struct ZeroWidthChangeSet;

    // Fake Merge impl to fulfill Store expectations
    impl Merge for ZeroWidthChangeSet {
        fn merge(&mut self, _other: Self) {}

        fn is_empty(&self) -> bool {
            false
        }
    }

    #[test]
    fn load_decodes_zero_width_changeset() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let mut bytes = TEST_MAGIC_BYTES.to_vec();
        // A single, well-formed frame with a zero-length payload loads and returns.
        bytes.extend_from_slice(&postcard::to_allocvec(&0u64).unwrap());
        fs::write(&file_path, bytes).expect("should write crafted store");

        let (_, changeset) = Store::<ZeroWidthChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
            .expect("zero-width changeset should load successfully");
        assert!(changeset.is_some());
    }

    #[test]
    fn load_roundtrips_at_varint_length_boundaries() {
        // The varint length prefix is 1 byte for values < 128 and 2 bytes for values >= 128.
        // Exercise both sides of that boundary: a payload of exactly 127 bytes (1-byte prefix)
        // and exactly 128 bytes (2-byte prefix).
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");

        // `TestChangeSet` (`BTreeSet<String>`) with one entry encodes as:
        // varint(1) [1 byte] + varint(str.len()) [1 byte, for str.len() < 128] + str bytes.
        // i.e., base_len = 2 bytes
        let base_len = postcard::to_allocvec(&TestChangeSet::from([String::new()]))
            .unwrap()
            .len();

        let mut store = Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).unwrap();
        let mut changesets = Vec::new();
        for target_len in [127_usize, 128] {
            let changeset = TestChangeSet::from(["x".repeat(target_len - base_len)]);
            assert_eq!(
                postcard::to_allocvec(&changeset).unwrap().len(),
                target_len,
                "test setup: payload should be exactly {target_len} bytes"
            );
            store.append(&changeset).unwrap();
            changesets.push(changeset);
        }
        drop(store);

        let (_, aggregated) = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path).unwrap();
        let expected = changesets
            .into_iter()
            .reduce(|mut acc, cs| {
                Merge::merge(&mut acc, cs);
                acc
            })
            .unwrap();
        assert_eq!(aggregated, Some(expected));
    }

    #[test]
    fn append_from_stale_handle_does_not_overwrite_existing_changeset() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let initial = TestChangeSet::from(["initial".to_string()]);
        let first_update = TestChangeSet::from(["first".to_string()]);
        let second_update = TestChangeSet::from(["other".to_string()]);

        let mut first =
            Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).expect("must create");
        first.append(&initial).expect("must append initial state");

        let (mut stale, _) = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
            .expect("must open second handle");

        first
            .append(&first_update)
            .expect("must append first update");
        stale
            .append(&second_update)
            .expect("must append from second handle");
        drop(first);
        drop(stale);

        let (_, recovered) = Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path)
            .expect("both appends must remain decodable");
        let mut expected = initial;
        expected.extend(first_update);
        expected.extend(second_update);
        assert_eq!(
            recovered,
            Some(expected),
            "a stale handle overwrote an append"
        );
    }

    // A failed append must roll back the partial frame so the file is left unchanged.
    //
    // The write is forced to fail by swapping the store's private file handle for a read-only
    // one: `write_all` then errors, the rollback (`set_len`) runs, and the original file is
    // untouched.
    #[test]
    fn append_failure_leaves_file_unchanged() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("db_file");
        let changeset = TestChangeSet::from(["one".to_string()]);

        let mut store =
            Store::<TestChangeSet>::create(&TEST_MAGIC_BYTES, &file_path).expect("must create");
        store.append(&changeset).expect("must append changeset");
        let bytes_before = fs::read(&file_path).expect("must read store file");

        // Replace the writable handle with a read-only one so the next write fails.
        store.db_file = fs::File::open(&file_path).expect("must open read-only handle");
        let result = store.append(&changeset);

        assert!(
            result.is_err(),
            "append through a read-only handle must fail"
        );
        let bytes_after = fs::read(&file_path).expect("must read store file");
        assert_eq!(
            bytes_before, bytes_after,
            "failed append left torn data behind"
        );

        // The store still loads and recovers the original changeset.
        let (_, recovered) =
            Store::<TestChangeSet>::load(&TEST_MAGIC_BYTES, &file_path).expect("must load");
        assert_eq!(recovered, Some(changeset));
    }
}
