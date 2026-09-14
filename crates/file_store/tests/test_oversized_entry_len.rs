use bdk_file_store::Store;
use std::collections::BTreeSet;

const MAGIC: &[u8] = b"bdk_test_magic";

#[test]
fn load_returns_error_on_oversized_length_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");

    let mut bytes = MAGIC.to_vec();
    bytes.push(1); // BTreeSet length: 1 element
    bytes.push(253); // bincode varint tag: u64 follows
    bytes.extend_from_slice(&u64::MAX.to_le_bytes()); // String length: u64::MAX
    std::fs::write(&path, &bytes).unwrap();

    let result = Store::<BTreeSet<String>>::load(MAGIC, &path);
    assert!(result.is_err(), "load should fail with an error, not panic");
}

#[test]
fn valid_store_still_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");

    let mut store = Store::<BTreeSet<String>>::create(MAGIC, &path).unwrap();
    store
        .append(&BTreeSet::from(["initial".to_string()]))
        .unwrap();
    store
        .append(&BTreeSet::from(["second".to_string()]))
        .unwrap();
    drop(store);

    let (_, recovered) = Store::<BTreeSet<String>>::load(MAGIC, &path).unwrap();
    assert_eq!(
        recovered,
        Some(BTreeSet::from([
            "initial".to_string(),
            "second".to_string()
        ]))
    );
}
