#![cfg(feature = "rusqlite")]
use bdk_chain::{keychain_txout, local_chain, tx_graph, ConfirmationBlockTime};
use bdk_testenv::persist_test_utils::{
    persist_anchors, persist_first_seen, persist_indexer_changeset, persist_last_evicted,
    persist_last_revealed, persist_last_seen, persist_local_chain_changeset, persist_spk_cache,
    persist_txgraph_changeset, persist_txouts, persist_txs,
};

#[test]
fn txgraph_is_persisted() {
    persist_txgraph_changeset::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn indexer_is_persisted() {
    persist_indexer_changeset::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            keychain_txout::ChangeSet::init_sqlite_tables(&db_tx)?;
            let changeset = keychain_txout::ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn local_chain_is_persisted() {
    persist_local_chain_changeset::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            local_chain::ChangeSet::init_sqlite_tables(&db_tx)?;
            let changeset = local_chain::ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn txouts_are_persisted() {
    persist_txouts::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn txs_are_persisted() {
    persist_txs::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn anchors_are_persisted() {
    persist_anchors::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn last_seen_is_persisted() {
    persist_last_seen::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn last_evicted_is_persisted() {
    persist_last_evicted::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn first_seen_is_persisted() {
    persist_first_seen::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(bdk_chain::rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(&db_tx)?;
            let changeset = tx_graph::ChangeSet::<ConfirmationBlockTime>::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn last_revealed_is_persisted() {
    persist_last_revealed::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            keychain_txout::ChangeSet::init_sqlite_tables(&db_tx)?;
            let changeset = keychain_txout::ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}

#[test]
fn spk_cache_is_persisted() {
    persist_spk_cache::<rusqlite::Connection, _, _, _>(
        "wallet.sqlite",
        |path| Ok(rusqlite::Connection::open(path)?),
        |db| {
            let db_tx = db.transaction()?;
            keychain_txout::ChangeSet::init_sqlite_tables(&db_tx)?;
            let changeset = keychain_txout::ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            Ok(changeset)
        },
        |db, changeset| {
            let db_tx = db.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            Ok(db_tx.commit()?)
        },
    );
}
