#![cfg(feature = "rusqlite")]
#[cfg(feature = "miniscript")]
use bdk_chain::keychain_txout;
use bdk_chain::{local_chain, tx_graph, Merge};
use bdk_testenv::persist_test_utils::*;
use core::fmt::Debug;
use rusqlite::Transaction;

#[test]
fn changesets_are_persisted() {
    #[track_caller]
    fn assert_sqlite_persists<C>(
        changesets: &[C],
        init_tables: fn(&Transaction) -> rusqlite::Result<()>,
        from_sqlite: fn(&Transaction) -> rusqlite::Result<C>,
        persist_to_sqlite: fn(&C, &Transaction) -> rusqlite::Result<()>,
    ) where
        C: Debug + PartialEq + Default + Merge + Clone,
    {
        let temp_dir = tempfile::tempdir().unwrap();
        assert_persist_changesets(
            || {
                let mut db = rusqlite::Connection::open(temp_dir.path().join("wallet.sqlite"))?;
                let db_tx = db.transaction()?;
                init_tables(&db_tx)?;
                db_tx.commit()?;
                Ok(db)
            },
            |db| {
                let db_tx = db.transaction()?;
                Ok(from_sqlite(&db_tx)?)
            },
            |db, changeset| {
                let db_tx = db.transaction()?;
                persist_to_sqlite(changeset, &db_tx)?;
                db_tx.commit()?;
                Ok(())
            },
            changesets,
        );
    }

    assert_sqlite_persists(
        &tx_graph_changesets(),
        tx_graph::ChangeSet::init_sqlite_tables,
        tx_graph::ChangeSet::from_sqlite,
        tx_graph::ChangeSet::persist_to_sqlite,
    );
    #[cfg(feature = "miniscript")]
    assert_sqlite_persists(
        &keychain_txout_changesets(),
        keychain_txout::ChangeSet::init_sqlite_tables,
        keychain_txout::ChangeSet::from_sqlite,
        keychain_txout::ChangeSet::persist_to_sqlite,
    );
    assert_sqlite_persists(
        &local_chain_changesets(),
        local_chain::ChangeSet::init_sqlite_tables,
        local_chain::ChangeSet::from_sqlite,
        local_chain::ChangeSet::persist_to_sqlite,
    );
}
