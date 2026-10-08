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
