#[cfg(unix)]
#[test]
fn new_store_permissions() {
    use std::{collections::BTreeSet, os::unix::fs::PermissionsExt};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.store");
    let _store = bdk_file_store::Store::<BTreeSet<String>>::create(b"test", &path).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;

    assert_eq!(mode & 0o077, 0, "store file mode: {mode:04o}");
}
