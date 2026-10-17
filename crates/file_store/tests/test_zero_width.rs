use bdk_file_store::Store;
use std::time::Duration;

const MAGIC: &[u8] = b"bdk_test_magic";

#[test]
fn load_terminates_on_zero_width_changeset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut bytes = MAGIC.to_vec();
    bytes.push(0xff); // any trailing byte
    std::fs::write(&path, &bytes).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = Store::<()>::load(MAGIC, &path);
        let _ = tx.send(result.is_ok());
    });
    // `()` decodes as zero bytes, so the file offset never advances and `load` never reaches EOF.
    assert!(
        rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "load did not terminate"
    );
}

#[test]
fn load_terminates_on_empty_zero_width_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    // No entries at all, just the magic bytes. `()` still decodes at EOF, so this hangs
    // without the offset check even though the file is well formed.
    std::fs::write(&path, MAGIC).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Store::<()>::load(MAGIC, &path).map(|(_, changeset)| changeset));
    });
    let changeset = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("load did not terminate")
        .expect("an empty store is not an error");
    assert!(changeset.is_none(), "an empty store has no changeset");
}
