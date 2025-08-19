#![cfg(feature = "rusqlite")]
use bdk_chain::{keychain_txout, local_chain, tx_graph, ConfirmationBlockTime};
use bdk_testenv::persist_test_utils::{
    persist_indexer_changeset, persist_local_chain_changeset, persist_txgraph_changeset,
};

fn tx_graph_changeset_init(
    db: &mut rusqlite::Connection,
) -> rusqlite::Result<tx_graph::ChangeSet<ConfirmationBlockTime>> {
    let db_tx = db.transaction()?;
    tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
    let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
    db_tx.commit()?;
    Ok(changeset)
}

fn tx_graph_changeset_persist(
    db: &mut rusqlite::Connection,
    changeset: &tx_graph::ChangeSet<ConfirmationBlockTime>,
) -> rusqlite::Result<()> {
    let db_tx = db.transaction()?;
    changeset.persist_to_sqlite(&db_tx)?;
    db_tx.commit()
}

fn keychain_txout_changeset_init(
    db: &mut rusqlite::Connection,
) -> rusqlite::Result<keychain_txout::ChangeSet> {
    let db_tx = db.transaction()?;
    keychain_txout::ChangeSet::init_sqlite_tables(&db_tx)?;
    let changeset = keychain_txout::ChangeSet::from_sqlite(&db_tx)?;
    db_tx.commit()?;
    Ok(changeset)
}

fn keychain_txout_changeset_persist(
    db: &mut rusqlite::Connection,
    changeset: &keychain_txout::ChangeSet,
) -> rusqlite::Result<()> {
    let db_tx = db.transaction()?;
    changeset.persist_to_sqlite(&db_tx)?;
    db_tx.commit()
}

fn local_chain_changeset_init(
    db: &mut rusqlite::Connection,
) -> rusqlite::Result<local_chain::ChangeSet> {
    let db_tx = db.transaction()?;
    local_chain::ChangeSet::init_sqlite_tables(&db_tx)?;
    let changeset = local_chain::ChangeSet::from_sqlite(&db_tx)?;
    db_tx.commit()?;
    Ok(changeset)
}

fn local_chain_changeset_persist(
    db: &mut rusqlite::Connection,
    changeset: &local_chain::ChangeSet,
) -> rusqlite::Result<()> {
    let db_tx = db.transaction()?;
    changeset.persist_to_sqlite(&db_tx)?;
    db_tx.commit()
}

#[test]
fn txgraph_is_persisted() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    Ok(persist_txgraph_changeset::<rusqlite::Connection, _, _, _>(
        || {
            Ok(rusqlite::Connection::open(
                temp_dir.path().join("wallet.sqlite"),
            )?)
        },
        |db| Ok(tx_graph_changeset_init(db)?),
        |db, changeset| Ok(tx_graph_changeset_persist(db, changeset)?),
    )?)
}

#[test]
fn indexer_is_persisted() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    Ok(persist_indexer_changeset::<rusqlite::Connection, _, _, _>(
        || {
            Ok(rusqlite::Connection::open(
                temp_dir.path().join("wallet.sqlite"),
            )?)
        },
        |db| Ok(keychain_txout_changeset_init(db)?),
        |db, changeset| Ok(keychain_txout_changeset_persist(db, changeset)?),
    )?)
}

#[test]
fn local_chain_is_persisted() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir().unwrap();
    Ok(persist_local_chain_changeset::<
        rusqlite::Connection,
        _,
        _,
        _,
    >(
        || {
            Ok(rusqlite::Connection::open(
                temp_dir.path().join("wallet.sqlite"),
            )?)
        },
        |db| Ok(local_chain_changeset_init(db)?),
        |db, changeset| Ok(local_chain_changeset_persist(db, changeset)?),
    )?)
}
