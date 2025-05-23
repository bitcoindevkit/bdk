use bdk_core::TxUpdate;

#[test]
fn test_empty() {
    assert!(
        TxUpdate::<()>::default().is_empty(),
        "Default `TxUpdate` must be empty"
    );
}
