use bdk_file_store::Store;
use std::collections::BTreeSet;

const MAGIC: &[u8] = b"bdk_test_magic";
type ChangeSet = BTreeSet<String>;

#[test]
fn append_through_second_handle_keeps_earlier_append() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");

    let mut first = Store::<ChangeSet>::create(MAGIC, &path).unwrap();
    first
        .append(&ChangeSet::from(["initial".to_string()]))
        .unwrap();

    // Second handle opened while the file ends after "initial".
    let (mut second, _) = Store::<ChangeSet>::load(MAGIC, &path).unwrap();

    first
        .append(&ChangeSet::from(["first".to_string()]))
        .unwrap();
    second
        .append(&ChangeSet::from(["other".to_string()]))
        .unwrap();
    drop((first, second));

    let (_, recovered) = Store::<ChangeSet>::load(MAGIC, &path).unwrap();
    let expected = ChangeSet::from(["initial".into(), "first".into(), "other".into()]);
    assert_eq!(recovered, Some(expected));
}
