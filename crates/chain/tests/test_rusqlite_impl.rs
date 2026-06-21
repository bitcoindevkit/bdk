#![cfg(feature = "rusqlite")]
use anyhow::anyhow;
use bdk_chain::{keychain_txout, local_chain, tx_graph, ConfirmationBlockTime};
use bdk_testenv::persist_test_utils::{
    assert_persist_changesets, keychain_txout_changesets, local_chain_changesets,
    tx_graph_changesets,
};

#[test]
fn txgraph_is_persisted() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    let changesets = tx_graph_changesets();
    assert_persist_changesets(
        || {
            let mut db = rusqlite::Connection::open(temp_dir.path().join("wallet.sqlite"))?;
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            db_tx.commit()?;
            Ok(db)
        },
        |db| {
            let db_tx = db.transaction()?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(())
        },
        &changesets,
    )
    .map_err(|err| anyhow!(err))
}

#[test]
fn indexer_is_persisted() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    let changesets = keychain_txout_changesets();
    assert_persist_changesets(
        || {
            let mut db = rusqlite::Connection::open(temp_dir.path().join("wallet.sqlite"))?;
            let db_tx = db.transaction()?;
            keychain_txout::ChangeSet::init_sqlite_tables(&db_tx)?;
            db_tx.commit()?;
            Ok(db)
        },
        |db| {
            let db_tx = db.transaction()?;
            let changeset = keychain_txout::ChangeSet::from_sqlite(&db_tx)?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(())
        },
        &changesets,
    )
    .map_err(|err| anyhow!(err))
}

#[test]
fn local_chain_is_persisted() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    let changesets = local_chain_changesets();
    assert_persist_changesets(
        || {
            let mut db = rusqlite::Connection::open(temp_dir.path().join("wallet.sqlite"))?;
            let db_tx = db.transaction()?;
            local_chain::ChangeSet::init_sqlite_tables(&db_tx)?;
            db_tx.commit()?;
            Ok(db)
        },
        |db| {
            let db_tx = db.transaction()?;
            let changeset = local_chain::ChangeSet::from_sqlite(&db_tx)?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(())
        },
        &changesets,
    )
    .map_err(|err| anyhow!(err))
}
