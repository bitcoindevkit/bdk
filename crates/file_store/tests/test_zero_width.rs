use bdk_file_store::Store;
use std::{sync::mpsc, time::Duration};

const MAGIC: &[u8] = b"bdk_test_magic";

/// Loads `Store::<()>` from a file containing `contents` and returns whether `load` terminated.
///
/// `()` is used because its encoding is zero bytes, so decoding it never advances the file offset.
/// The load runs on a detached thread so a hang fails the assertion instead of the whole test run.
fn load_terminates(contents: &[u8]) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    std::fs::write(&path, contents).unwrap();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Store::<()>::load(MAGIC, &path).is_ok());
    });
    rx.recv_timeout(Duration::from_secs(5)).is_ok()
}

#[test]
fn load_terminates_on_zero_width_changeset() {
    let mut contents = MAGIC.to_vec();
    contents.push(0xff);
    assert!(load_terminates(&contents), "load did not terminate");
}

// `Store::<()>` cannot tell a magic-only file from a corrupt one, since every decode of `()`
// succeeds without consuming bytes. An error is therefore the expected outcome; what matters is
// that `load` no longer hangs.
#[test]
fn load_terminates_on_magic_only_zero_width_store() {
    assert!(load_terminates(MAGIC), "load did not terminate");
}
