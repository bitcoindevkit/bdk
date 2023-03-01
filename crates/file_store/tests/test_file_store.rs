use bdk_chain::{
    bitcoin::Transaction,
    keychain::{KeychainChangeSet, KeychainTracker},
    TxHeight,
};
use bdk_file_store::{FileError, IterError, KeychainStore, MAGIC_BYTES, MAGIC_BYTES_LEN};
use serde;
use std::{
    format,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    vec::Vec,
};

struct TempPath(PathBuf);

impl TempPath {
    fn new() -> Self {
        let now = std::time::UNIX_EPOCH
            .elapsed()
            .expect("must get epoch")
            .as_nanos();
        let mut file_path = std::env::temp_dir();
        file_path.push(format!("bdk_test_{}", now));
        Self(file_path)
    }

    fn open(&self) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(self.0.as_path())
            .expect("must open")
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(self.0.as_path()) {
            if e.kind() != std::io::ErrorKind::NotFound {
                panic!("remove file unexpected error: {}", e);
            }
        };
    }
}

#[derive(
    Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
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
    let path = TempPath::new();
    path.open()
        .write_all(&MAGIC_BYTES[..MAGIC_BYTES_LEN - 1])
        .expect("should write");

    match KeychainStore::<TestKeychain, TxHeight, Transaction>::new(path.open()) {
        Err(FileError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
        unexpected => panic!("unexpected result: {:?}", unexpected),
    };
}

#[test]
fn new_fails_if_magic_bytes_are_invalid() {
    let invalid_magic_mnemonic = "ldkfs0000000";

    let path = TempPath::new();
    path.open()
        .write_all(invalid_magic_mnemonic.as_bytes())
        .expect("should write");

    match KeychainStore::<TestKeychain, TxHeight, Transaction>::new(path.open()) {
        Err(FileError::InvalidMagicBytes(b)) => {
            assert_eq!(b, invalid_magic_mnemonic.as_bytes())
        }
        unexpected => panic!("unexpected result: {:?}", unexpected),
    };
}

#[test]
fn append_changeset_truncates_invalid_bytes() {
    use bdk_chain::miniscript;
    use core::str::FromStr;
    // initial data to write to file (magic bytes + invalid data)
    let mut data = [255_u8; 2000];
    data[..MAGIC_BYTES_LEN].copy_from_slice(&MAGIC_BYTES);

    let descriptor = miniscript::Descriptor::from_str("tr([73c5da0a/86'/0'/0']xpub6BgBgsespWvERF3LHQu6CnqdvfEvtMcQjYrcRzx53QJjSxarj2afYWcLteoGVky7D3UKDP9QyrLprQ3VCECoY49yfdDEHGCtMMj92pReUsQ/0/*)#rg247h69").unwrap();
    let mut tracker = KeychainTracker::<TestKeychain, TxHeight, Transaction>::default();
    tracker.add_keychain(TestKeychain::External, descriptor);
    let changeset = KeychainChangeSet {
        derivation_indices: tracker
            .txout_index
            .reveal_to_target(&TestKeychain::External, 21)
            .1,
        chain_graph: Default::default(),
    };

    let path = TempPath::new();
    path.open().write_all(&data).expect("should write");

    let mut store = KeychainStore::<TestKeychain, TxHeight, Transaction>::new(path.open())
        .expect("should open");
    match store.iter_changesets().expect("seek should succeed").next() {
        Some(Err(IterError::Bincode(_))) => {}
        unexpected_res => panic!("unexpected result: {:?}", unexpected_res),
    }

    store.append_changeset(&changeset).expect("should append");

    drop(store);

    let got_bytes = {
        let mut buf = Vec::new();
        path.open().read_to_end(&mut buf).expect("should read");
        buf
    };

    let expected_bytes = {
        let mut buf = MAGIC_BYTES.to_vec();
        bincode::encode_into_std_write(
            bincode::serde::Compat(&changeset),
            &mut buf,
            bincode::config::standard(),
        )
        .expect("should encode");
        buf
    };

    assert_eq!(got_bytes, expected_bytes);
}
