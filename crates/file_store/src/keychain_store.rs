//! Module for persisting data on disk.
//!
//! The star of the show is [`KeychainStore`], which maintains an append-only file of
//! [`KeychainChangeSet`]s which can be used to restore a [`KeychainTracker`].
use bdk_chain::{
    keychain::{KeychainChangeSet, KeychainTracker},
    sparse_chain,
};
use bincode::Options;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::Path,
};

use crate::{bincode_options, EntryIter, IterError};

/// BDK File Store magic bytes length.
const MAGIC_BYTES_LEN: usize = 12;

/// BDK File Store magic bytes.
const MAGIC_BYTES: [u8; MAGIC_BYTES_LEN] = [98, 100, 107, 102, 115, 48, 48, 48, 48, 48, 48, 48];

/// Persists an append only list of `KeychainChangeSet<K,P>` to a single file.
/// [`KeychainChangeSet<K,P>`] record the changes made to a [`KeychainTracker<K,P>`].
#[derive(Debug)]
pub struct KeychainStore<K, P> {
    db_file: File,
    changeset_type_params: core::marker::PhantomData<(K, P)>,
}

impl<K, P> KeychainStore<K, P>
where
    K: Ord + Clone + core::fmt::Debug,
    P: sparse_chain::ChainPosition,
    KeychainChangeSet<K, P>: serde::Serialize + serde::de::DeserializeOwned,
{
    /// Creates a new store from a [`File`].
    ///
    /// The file must have been opened with read and write permissions.
    ///
    /// [`File`]: std::fs::File
    pub fn new(mut file: File) -> Result<Self, FileError> {
        file.rewind()?;

        let mut magic_bytes = [0_u8; MAGIC_BYTES_LEN];
        file.read_exact(&mut magic_bytes)?;

        if magic_bytes != MAGIC_BYTES {
            return Err(FileError::InvalidMagicBytes(magic_bytes));
        }

        Ok(Self {
            db_file: file,
            changeset_type_params: Default::default(),
        })
    }

    /// Creates or loads a store from `db_path`. If no file exists there, it will be created.
    pub fn new_from_path<D: AsRef<Path>>(db_path: D) -> Result<Self, FileError> {
        let already_exists = db_path.as_ref().exists();

        let mut db_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(db_path)?;

        if !already_exists {
            db_file.write_all(&MAGIC_BYTES)?;
        }

        Self::new(db_file)
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
    pub fn iter_changesets(&mut self) -> Result<EntryIter<KeychainChangeSet<K, P>>, io::Error> {
        Ok(EntryIter::new(MAGIC_BYTES_LEN as u64, &mut self.db_file))
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
    pub fn aggregate_changeset(&mut self) -> (KeychainChangeSet<K, P>, Result<(), IterError>) {
        let mut changeset = KeychainChangeSet::default();
        let result = (|| {
            let iter_changeset = self.iter_changesets()?;
            for next_changeset in iter_changeset {
                changeset.append(next_changeset?);
            }
            Ok(())
        })();

        (changeset, result)
    }

    /// Reads and applies all the changesets stored sequentially to the tracker, stopping when it fails
    /// to read the next one.
    ///
    /// **WARNING**: This method changes the write position of the underlying file. The next
    /// changeset will be written over the erroring entry (or the end of the file if none existed).
    pub fn load_into_keychain_tracker(
        &mut self,
        tracker: &mut KeychainTracker<K, P>,
    ) -> Result<(), IterError> {
        for changeset in self.iter_changesets()? {
            tracker.apply_changeset(changeset?)
        }
        Ok(())
    }

    /// Append a new changeset to the file and truncate the file to the end of the appended changeset.
    ///
    /// The truncation is to avoid the possibility of having a valid but inconsistent changeset
    /// directly after the appended changeset.
    pub fn append_changeset(
        &mut self,
        changeset: &KeychainChangeSet<K, P>,
    ) -> Result<(), io::Error> {
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

        // We want to make sure that derivation indices changes are written to disk as soon as
        // possible, so you know about the write failure before you give out the address in the application.
        if !changeset.derivation_indices.is_empty() {
            self.db_file.sync_data()?;
        }

        Ok(())
    }
}

/// Error that occurs due to problems encountered with the file.
#[derive(Debug)]
pub enum FileError {
    /// IO error, this may mean that the file is too short.
    Io(io::Error),
    /// Magic bytes do not match what is expected.
    InvalidMagicBytes([u8; MAGIC_BYTES_LEN]),
}

impl core::fmt::Display for FileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error trying to read file: {}", e),
            Self::InvalidMagicBytes(b) => write!(
                f,
                "file has invalid magic bytes: expected={:?} got={:?}",
                MAGIC_BYTES, b
            ),
        }
    }
}

impl From<io::Error> for FileError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl std::error::Error for FileError {}

#[cfg(test)]
mod test {
    use super::*;
    use bdk_chain::{
        keychain::{DerivationAdditions, KeychainChangeSet},
        TxHeight,
    };
    use bincode::DefaultOptions;
    use std::{
        io::{Read, Write},
        vec::Vec,
    };
    use tempfile::NamedTempFile;
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

    #[test]
    fn magic_bytes() {
        assert_eq!(&MAGIC_BYTES, "bdkfs0000000".as_bytes());
    }

    #[test]
    fn new_fails_if_file_is_too_short() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&MAGIC_BYTES[..MAGIC_BYTES_LEN - 1])
            .expect("should write");

        match KeychainStore::<TestKeychain, TxHeight>::new(file.reopen().unwrap()) {
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

        match KeychainStore::<TestKeychain, TxHeight>::new(file.reopen().unwrap()) {
            Err(FileError::InvalidMagicBytes(b)) => {
                assert_eq!(b, invalid_magic_bytes.as_bytes())
            }
            unexpected => panic!("unexpected result: {:?}", unexpected),
        };
    }

    #[test]
    fn append_changeset_truncates_invalid_bytes() {
        // initial data to write to file (magic bytes + invalid data)
        let mut data = [255_u8; 2000];
        data[..MAGIC_BYTES_LEN].copy_from_slice(&MAGIC_BYTES);

        let changeset = KeychainChangeSet {
            derivation_indices: DerivationAdditions(
                vec![(TestKeychain::External, 42)].into_iter().collect(),
            ),
            chain_graph: Default::default(),
        };

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&data).expect("should write");

        let mut store = KeychainStore::<TestKeychain, TxHeight>::new(file.reopen().unwrap())
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
            let mut buf = MAGIC_BYTES.to_vec();
            DefaultOptions::new()
                .with_varint_encoding()
                .serialize_into(&mut buf, &changeset)
                .expect("should encode");
            buf
        };

        assert_eq!(got_bytes, expected_bytes);
    }
}
