//! Support for persisting `bdk_chain` structures to SQLite using [`rusqlite`].

use crate::*;
use core::str::FromStr;

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use bitcoin::consensus::{Decodable, Encodable};
use rusqlite;
use rusqlite::named_params;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::OptionalExtension;
use rusqlite::Transaction;

/// Table name for schemas.
pub const SCHEMAS_TABLE_NAME: &str = "bdk_schemas";

/// Initialize the schema table.
fn init_schemas_table(db_tx: &Transaction) -> rusqlite::Result<()> {
    let sql = format!("CREATE TABLE IF NOT EXISTS {}( name TEXT PRIMARY KEY NOT NULL, version INTEGER NOT NULL ) STRICT", SCHEMAS_TABLE_NAME);
    db_tx.execute(&sql, ())?;
    Ok(())
}

/// Get schema version of `schema_name`.
fn schema_version(db_tx: &Transaction, schema_name: &str) -> rusqlite::Result<Option<u32>> {
    let sql = format!(
        "SELECT version FROM {} WHERE name=:name",
        SCHEMAS_TABLE_NAME
    );
    db_tx
        .query_row(&sql, named_params! { ":name": schema_name }, |row| {
            row.get::<_, u32>("version")
        })
        .optional()
}

/// Set the `schema_version` of `schema_name`.
fn set_schema_version(
    db_tx: &Transaction,
    schema_name: &str,
    schema_version: u32,
) -> rusqlite::Result<()> {
    let sql = format!(
        "REPLACE INTO {}(name, version) VALUES(:name, :version)",
        SCHEMAS_TABLE_NAME,
    );
    db_tx.execute(
        &sql,
        named_params! { ":name": schema_name, ":version": schema_version },
    )?;
    Ok(())
}

/// Runs logic that initializes/migrates the table schemas.
pub fn migrate_schema(
    db_tx: &Transaction,
    schema_name: &str,
    versioned_scripts: &[&str],
) -> rusqlite::Result<()> {
    init_schemas_table(db_tx)?;
    let current_version = schema_version(db_tx, schema_name)?;
    let exec_from = current_version.map_or(0_usize, |v| v as usize + 1);
    let scripts_to_exec = versioned_scripts.iter().enumerate().skip(exec_from);
    for (version, script) in scripts_to_exec {
        set_schema_version(db_tx, schema_name, version as u32)?;
        db_tx.execute_batch(script)?;
    }
    Ok(())
}

impl FromSql for Impl<bitcoin::Txid> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::Txid::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Impl<bitcoin::Txid> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromSql for Impl<bitcoin::BlockHash> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::BlockHash::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Impl<bitcoin::BlockHash> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

#[cfg(feature = "miniscript")]
impl FromSql for Impl<DescriptorId> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        DescriptorId::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

#[cfg(feature = "miniscript")]
impl ToSql for Impl<DescriptorId> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromSql for Impl<bitcoin::Transaction> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::Transaction::consensus_decode_from_finite_reader(&mut value.as_bytes()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Impl<bitcoin::Transaction> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let mut bytes = Vec::<u8>::new();
        self.consensus_encode(&mut bytes).map_err(to_sql_error)?;
        Ok(bytes.into())
    }
}

impl FromSql for Impl<bitcoin::ScriptBuf> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(bitcoin::Script::from_bytes(value.as_bytes()?)
            .to_owned()
            .into())
    }
}

impl ToSql for Impl<bitcoin::ScriptBuf> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_bytes().into())
    }
}

impl FromSql for Impl<bitcoin::Amount> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(bitcoin::Amount::from_sat(value.as_i64()?.try_into().map_err(from_sql_error)?).into())
    }
}

impl ToSql for Impl<bitcoin::Amount> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let amount: i64 = self.to_sat().try_into().map_err(to_sql_error)?;
        Ok(amount.into())
    }
}

#[cfg(feature = "miniscript")]
impl FromSql for Impl<miniscript::Descriptor<miniscript::DescriptorPublicKey>> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        miniscript::Descriptor::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

#[cfg(feature = "miniscript")]
impl ToSql for Impl<miniscript::Descriptor<miniscript::DescriptorPublicKey>> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromSql for Impl<bitcoin::Network> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::Network::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Impl<bitcoin::Network> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

fn from_sql_error<E: std::error::Error + Send + Sync + 'static>(err: E) -> FromSqlError {
    FromSqlError::Other(Box::new(err))
}

fn to_sql_error<E: std::error::Error + Send + Sync + 'static>(err: E) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(err))
}

impl tx_graph::ChangeSet<ConfirmationBlockTime> {
    /// Schema name for [`tx_graph::ChangeSet`].
    pub const SCHEMA_NAME: &'static str = "bdk_txgraph";
    /// Name of table that stores full transactions and `last_seen` timestamps.
    pub const TXS_TABLE_NAME: &'static str = "bdk_txs";
    /// Name of table that stores floating txouts.
    pub const TXOUTS_TABLE_NAME: &'static str = "bdk_txouts";
    /// Name of table that stores [`Anchor`]s.
    pub const ANCHORS_TABLE_NAME: &'static str = "bdk_anchors";

    /// Get v0 of sqlite [tx_graph::ChangeSet] schema
    pub fn schema_v0() -> String {
        // full transactions
        let create_txs_table = format!(
            "CREATE TABLE {} ( \
            txid TEXT PRIMARY KEY NOT NULL, \
            raw_tx BLOB, \
            last_seen INTEGER \
            ) STRICT",
            Self::TXS_TABLE_NAME,
        );
        // floating txouts
        let create_txouts_table = format!(
            "CREATE TABLE {} ( \
            txid TEXT NOT NULL, \
            vout INTEGER NOT NULL, \
            value INTEGER NOT NULL, \
            script BLOB NOT NULL, \
            PRIMARY KEY (txid, vout) \
            ) STRICT",
            Self::TXOUTS_TABLE_NAME,
        );
        // anchors
        let create_anchors_table = format!(
            "CREATE TABLE {} ( \
            txid TEXT NOT NULL REFERENCES {} (txid), \
            block_height INTEGER NOT NULL, \
            block_hash TEXT NOT NULL, \
            anchor BLOB NOT NULL, \
            PRIMARY KEY (txid, block_height, block_hash) \
            ) STRICT",
            Self::ANCHORS_TABLE_NAME,
            Self::TXS_TABLE_NAME,
        );

        format!("{create_txs_table}; {create_txouts_table}; {create_anchors_table}")
    }

    /// Get v1 of sqlite [tx_graph::ChangeSet] schema
    pub fn schema_v1() -> String {
        let add_confirmation_time_column = format!(
            "ALTER TABLE {} ADD COLUMN confirmation_time INTEGER DEFAULT -1 NOT NULL",
            Self::ANCHORS_TABLE_NAME,
        );
        let extract_confirmation_time_from_anchor_column = format!(
            "UPDATE {} SET confirmation_time = json_extract(anchor, '$.confirmation_time')",
            Self::ANCHORS_TABLE_NAME,
        );
        let drop_anchor_column = format!(
            "ALTER TABLE {} DROP COLUMN anchor",
            Self::ANCHORS_TABLE_NAME,
        );
        format!("{add_confirmation_time_column}; {extract_confirmation_time_from_anchor_column}; {drop_anchor_column}")
    }

    /// Get v2 of sqlite [tx_graph::ChangeSet] schema
    pub fn schema_v2() -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN last_evicted INTEGER",
            Self::TXS_TABLE_NAME,
        )
    }

    /// Initialize sqlite tables.
    pub fn init_sqlite_tables(db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        migrate_schema(
            db_tx,
            Self::SCHEMA_NAME,
            &[&Self::schema_v0(), &Self::schema_v1(), &Self::schema_v2()],
        )
    }

    /// Construct a [`TxGraph`] from an sqlite database.
    ///
    /// Remember to call [`Self::init_sqlite_tables`] beforehand.
    pub fn from_sqlite(db_tx: &rusqlite::Transaction) -> rusqlite::Result<Self> {
        let mut changeset = Self::default();

        let mut statement = db_tx.prepare(&format!(
            "SELECT txid, raw_tx, last_seen, last_evicted FROM {}",
            Self::TXS_TABLE_NAME,
        ))?;
        let row_iter = statement.query_map([], |row| {
            Ok((
                row.get::<_, Impl<bitcoin::Txid>>("txid")?,
                row.get::<_, Option<Impl<bitcoin::Transaction>>>("raw_tx")?,
                row.get::<_, Option<u64>>("last_seen")?,
                row.get::<_, Option<u64>>("last_evicted")?,
            ))
        })?;
        for row in row_iter {
            let (Impl(txid), tx, last_seen, last_evicted) = row?;
            if let Some(Impl(tx)) = tx {
                changeset.txs.insert(Arc::new(tx));
            }
            if let Some(last_seen) = last_seen {
                changeset.last_seen.insert(txid, last_seen);
            }
            if let Some(last_evicted) = last_evicted {
                changeset.last_evicted.insert(txid, last_evicted);
            }
        }

        let mut statement = db_tx.prepare(&format!(
            "SELECT txid, vout, value, script FROM {}",
            Self::TXOUTS_TABLE_NAME,
        ))?;
        let row_iter = statement.query_map([], |row| {
            Ok((
                row.get::<_, Impl<bitcoin::Txid>>("txid")?,
                row.get::<_, u32>("vout")?,
                row.get::<_, Impl<bitcoin::Amount>>("value")?,
                row.get::<_, Impl<bitcoin::ScriptBuf>>("script")?,
            ))
        })?;
        for row in row_iter {
            let (Impl(txid), vout, Impl(value), Impl(script_pubkey)) = row?;
            changeset.txouts.insert(
                bitcoin::OutPoint { txid, vout },
                bitcoin::TxOut {
                    value,
                    script_pubkey,
                },
            );
        }

        let mut statement = db_tx.prepare(&format!(
            "SELECT block_hash, block_height, confirmation_time, txid FROM {}",
            Self::ANCHORS_TABLE_NAME,
        ))?;
        let row_iter = statement.query_map([], |row| {
            Ok((
                row.get::<_, Impl<bitcoin::BlockHash>>("block_hash")?,
                row.get::<_, u32>("block_height")?,
                row.get::<_, u64>("confirmation_time")?,
                row.get::<_, Impl<bitcoin::Txid>>("txid")?,
            ))
        })?;
        for row in row_iter {
            let (hash, height, confirmation_time, Impl(txid)) = row?;
            changeset.anchors.insert((
                ConfirmationBlockTime {
                    block_id: BlockId::from((&height, &hash.0)),
                    confirmation_time,
                },
                txid,
            ));
        }

        Ok(changeset)
    }

    /// Persist `changeset` to the sqlite database.
    ///
    /// Remember to call [`Self::init_sqlite_tables`] beforehand.
    pub fn persist_to_sqlite(&self, db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        let mut statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(txid, raw_tx) VALUES(:txid, :raw_tx) ON CONFLICT(txid) DO UPDATE SET raw_tx=:raw_tx",
            Self::TXS_TABLE_NAME,
        ))?;
        for tx in &self.txs {
            statement.execute(named_params! {
                ":txid": Impl(tx.compute_txid()),
                ":raw_tx": Impl(tx.as_ref().clone()),
            })?;
        }

        let mut statement = db_tx
            .prepare_cached(&format!(
                "INSERT INTO {}(txid, last_seen) VALUES(:txid, :last_seen) ON CONFLICT(txid) DO UPDATE SET last_seen=:last_seen",
                Self::TXS_TABLE_NAME,
            ))?;
        for (&txid, &last_seen) in &self.last_seen {
            let checked_time = last_seen.to_sql()?;
            statement.execute(named_params! {
                ":txid": Impl(txid),
                ":last_seen": Some(checked_time),
            })?;
        }

        let mut statement = db_tx
            .prepare_cached(&format!(
                "INSERT INTO {}(txid, last_evicted) VALUES(:txid, :last_evicted) ON CONFLICT(txid) DO UPDATE SET last_evicted=:last_evicted",
                Self::TXS_TABLE_NAME,
            ))?;
        for (&txid, &last_evicted) in &self.last_evicted {
            let checked_time = last_evicted.to_sql()?;
            statement.execute(named_params! {
                ":txid": Impl(txid),
                ":last_evicted": Some(checked_time),
            })?;
        }

        let mut statement = db_tx.prepare_cached(&format!(
            "REPLACE INTO {}(txid, vout, value, script) VALUES(:txid, :vout, :value, :script)",
            Self::TXOUTS_TABLE_NAME,
        ))?;
        for (op, txo) in &self.txouts {
            statement.execute(named_params! {
                ":txid": Impl(op.txid),
                ":vout": op.vout,
                ":value": Impl(txo.value),
                ":script": Impl(txo.script_pubkey.clone()),
            })?;
        }

        let mut statement = db_tx.prepare_cached(&format!(
            "REPLACE INTO {}(txid, block_height, block_hash, confirmation_time) VALUES(:txid, :block_height, :block_hash, :confirmation_time)",
            Self::ANCHORS_TABLE_NAME,
        ))?;
        let mut statement_txid = db_tx.prepare_cached(&format!(
            "INSERT OR IGNORE INTO {}(txid) VALUES(:txid)",
            Self::TXS_TABLE_NAME,
        ))?;
        for (anchor, txid) in &self.anchors {
            let anchor_block = anchor.anchor_block();
            statement_txid.execute(named_params! {
                ":txid": Impl(*txid)
            })?;
            statement.execute(named_params! {
                ":txid": Impl(*txid),
                ":block_height": anchor_block.height,
                ":block_hash": Impl(anchor_block.hash),
                ":confirmation_time": anchor.confirmation_time,
            })?;
        }

        Ok(())
    }
}

impl local_chain::ChangeSet {
    /// Schema name for the changeset.
    pub const SCHEMA_NAME: &'static str = "bdk_localchain";
    /// Name of sqlite table that stores blocks of [`LocalChain`](local_chain::LocalChain).
    pub const BLOCKS_TABLE_NAME: &'static str = "bdk_blocks";

    /// Get v0 of sqlite [local_chain::ChangeSet] schema
    pub fn schema_v0() -> String {
        // blocks
        format!(
            "CREATE TABLE {} ( \
            block_height INTEGER PRIMARY KEY NOT NULL, \
            block_hash TEXT NOT NULL \
            ) STRICT",
            Self::BLOCKS_TABLE_NAME,
        )
    }

    /// Initialize sqlite tables for persisting [`local_chain::LocalChain`].
    pub fn init_sqlite_tables(db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        migrate_schema(db_tx, Self::SCHEMA_NAME, &[&Self::schema_v0()])
    }

    /// Construct a [`LocalChain`](local_chain::LocalChain) from sqlite database.
    ///
    /// Remember to call [`Self::init_sqlite_tables`] beforehand.
    pub fn from_sqlite(db_tx: &rusqlite::Transaction) -> rusqlite::Result<Self> {
        let mut changeset = Self::default();

        let mut statement = db_tx.prepare(&format!(
            "SELECT block_height, block_hash FROM {}",
            Self::BLOCKS_TABLE_NAME,
        ))?;
        let row_iter = statement.query_map([], |row| {
            Ok((
                row.get::<_, u32>("block_height")?,
                row.get::<_, Impl<bitcoin::BlockHash>>("block_hash")?,
            ))
        })?;
        for row in row_iter {
            let (height, Impl(hash)) = row?;
            changeset.blocks.insert(height, Some(hash));
        }

        Ok(changeset)
    }

    /// Persist `changeset` to the sqlite database.
    ///
    /// Remember to call [`Self::init_sqlite_tables`] beforehand.
    pub fn persist_to_sqlite(&self, db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        let mut replace_statement = db_tx.prepare_cached(&format!(
            "REPLACE INTO {}(block_height, block_hash) VALUES(:block_height, :block_hash)",
            Self::BLOCKS_TABLE_NAME,
        ))?;
        let mut delete_statement = db_tx.prepare_cached(&format!(
            "DELETE FROM {} WHERE block_height=:block_height",
            Self::BLOCKS_TABLE_NAME,
        ))?;
        for (&height, &hash) in &self.blocks {
            match hash {
                Some(hash) => replace_statement.execute(named_params! {
                    ":block_height": height,
                    ":block_hash": Impl(hash),
                })?,
                None => delete_statement.execute(named_params! {
                    ":block_height": height,
                })?,
            };
        }

        Ok(())
    }
}

#[cfg(feature = "miniscript")]
impl keychain_txout::ChangeSet {
    /// Schema name for the changeset.
    pub const SCHEMA_NAME: &'static str = "bdk_keychaintxout";
    /// Name for table that stores last revealed indices per descriptor id.
    pub const LAST_REVEALED_TABLE_NAME: &'static str = "bdk_descriptor_last_revealed";

    /// Get v0 of sqlite [keychain_txout::ChangeSet] schema
    pub fn schema_v0() -> String {
        format!(
            "CREATE TABLE {} ( \
            descriptor_id TEXT PRIMARY KEY NOT NULL, \
            last_revealed INTEGER NOT NULL \
            ) STRICT",
            Self::LAST_REVEALED_TABLE_NAME,
        )
    }

    /// Initialize sqlite tables for persisting
    /// [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex).
    pub fn init_sqlite_tables(db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        migrate_schema(db_tx, Self::SCHEMA_NAME, &[&Self::schema_v0()])
    }

    /// Construct [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex) from sqlite database
    /// and given parameters.
    ///
    /// Remember to call [`Self::init_sqlite_tables`] beforehand.
    pub fn from_sqlite(db_tx: &rusqlite::Transaction) -> rusqlite::Result<Self> {
        let mut changeset = Self::default();

        let mut statement = db_tx.prepare(&format!(
            "SELECT descriptor_id, last_revealed FROM {}",
            Self::LAST_REVEALED_TABLE_NAME,
        ))?;
        let row_iter = statement.query_map([], |row| {
            Ok((
                row.get::<_, Impl<DescriptorId>>("descriptor_id")?,
                row.get::<_, u32>("last_revealed")?,
            ))
        })?;
        for row in row_iter {
            let (Impl(descriptor_id), last_revealed) = row?;
            changeset.last_revealed.insert(descriptor_id, last_revealed);
        }

        Ok(changeset)
    }

    /// Persist `changeset` to the sqlite database.
    ///
    /// Remember to call [`Self::init_sqlite_tables`] beforehand.
    pub fn persist_to_sqlite(&self, db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        let mut statement = db_tx.prepare_cached(&format!(
            "REPLACE INTO {}(descriptor_id, last_revealed) VALUES(:descriptor_id, :last_revealed)",
            Self::LAST_REVEALED_TABLE_NAME,
        ))?;
        for (&descriptor_id, &last_revealed) in &self.last_revealed {
            statement.execute(named_params! {
                ":descriptor_id": Impl(descriptor_id),
                ":last_revealed": last_revealed,
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use bdk_testenv::{anyhow, hash};
    use bitcoin::{absolute, transaction, TxIn, TxOut};

    #[test]
    fn can_persist_anchors_and_txs_independently() -> anyhow::Result<()> {
        type ChangeSet = tx_graph::ChangeSet<ConfirmationBlockTime>;
        let mut conn = rusqlite::Connection::open_in_memory()?;

        // init tables
        {
            let db_tx = conn.transaction()?;
            ChangeSet::init_sqlite_tables(&db_tx)?;
            db_tx.commit()?;
        }

        let tx = bitcoin::Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn::default()],
            output: vec![TxOut::NULL],
        };
        let tx = Arc::new(tx);
        let txid = tx.compute_txid();
        let anchor = ConfirmationBlockTime {
            block_id: BlockId {
                height: 21,
                hash: hash!("anchor"),
            },
            confirmation_time: 1342,
        };

        // First persist the anchor
        {
            let changeset = ChangeSet {
                anchors: [(anchor, txid)].into(),
                ..Default::default()
            };
            let db_tx = conn.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            db_tx.commit()?;
        }

        // Now persist the tx
        {
            let changeset = ChangeSet {
                txs: [tx.clone()].into(),
                ..Default::default()
            };
            let db_tx = conn.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            db_tx.commit()?;
        }

        // Loading changeset from sqlite should succeed
        {
            let db_tx = conn.transaction()?;
            let changeset = ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            assert!(changeset.txs.contains(&tx));
            assert!(changeset.anchors.contains(&(anchor, txid)));
        }

        Ok(())
    }

    #[test]
    fn v0_to_v2_schema_migration_is_backward_compatible() -> anyhow::Result<()> {
        type ChangeSet = tx_graph::ChangeSet<ConfirmationBlockTime>;
        let mut conn = rusqlite::Connection::open_in_memory()?;

        // Create initial database with v0 sqlite schema
        {
            let db_tx = conn.transaction()?;
            migrate_schema(&db_tx, ChangeSet::SCHEMA_NAME, &[&ChangeSet::schema_v0()])?;
            db_tx.commit()?;
        }

        let tx = bitcoin::Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn::default()],
            output: vec![TxOut::NULL],
        };
        let tx = Arc::new(tx);
        let txid = tx.compute_txid();
        let anchor = ConfirmationBlockTime {
            block_id: BlockId {
                height: 21,
                hash: hash!("anchor"),
            },
            confirmation_time: 1342,
        };

        // Persist anchor with v0 sqlite schema
        {
            let changeset = ChangeSet {
                anchors: [(anchor, txid)].into(),
                ..Default::default()
            };
            let mut statement = conn.prepare_cached(&format!(
                "REPLACE INTO {} (txid, block_height, block_hash, anchor)
                 VALUES(
                    :txid,
                    :block_height,
                    :block_hash,
                    jsonb('{{
                        \"block_id\": {{\"height\": {},\"hash\":\"{}\"}},
                        \"confirmation_time\": {}
                    }}')
                 )",
                ChangeSet::ANCHORS_TABLE_NAME,
                anchor.block_id.height,
                anchor.block_id.hash,
                anchor.confirmation_time,
            ))?;
            let mut statement_txid = conn.prepare_cached(&format!(
                "INSERT OR IGNORE INTO {}(txid) VALUES(:txid)",
                ChangeSet::TXS_TABLE_NAME,
            ))?;
            for (anchor, txid) in &changeset.anchors {
                let anchor_block = anchor.anchor_block();
                statement_txid.execute(named_params! {
                    ":txid": Impl(*txid)
                })?;
                match statement.execute(named_params! {
                    ":txid": Impl(*txid),
                    ":block_height": anchor_block.height,
                    ":block_hash": Impl(anchor_block.hash),
                }) {
                    Ok(updated) => assert_eq!(updated, 1),
                    Err(err) => panic!("update failed: {}", err),
                }
            }
        }

        // Apply v1 & v2 sqlite schema to tables with data
        {
            let db_tx = conn.transaction()?;
            migrate_schema(
                &db_tx,
                ChangeSet::SCHEMA_NAME,
                &[
                    &ChangeSet::schema_v0(),
                    &ChangeSet::schema_v1(),
                    &ChangeSet::schema_v2(),
                ],
            )?;
            db_tx.commit()?;
        }

        // Loading changeset from sqlite should succeed
        {
            let db_tx = conn.transaction()?;
            let changeset = ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            assert!(changeset.anchors.contains(&(anchor, txid)));
        }

        Ok(())
    }

    #[test]
    fn can_persist_last_evicted() -> anyhow::Result<()> {
        use bitcoin::hashes::Hash;

        type ChangeSet = tx_graph::ChangeSet<ConfirmationBlockTime>;
        let mut conn = rusqlite::Connection::open_in_memory()?;

        // Init tables
        {
            let db_tx = conn.transaction()?;
            ChangeSet::init_sqlite_tables(&db_tx)?;
            db_tx.commit()?;
        }

        let txid = bitcoin::Txid::all_zeros();
        let last_evicted = 100;

        // Persist `last_evicted`
        {
            let changeset = ChangeSet {
                last_evicted: [(txid, last_evicted)].into(),
                ..Default::default()
            };
            let db_tx = conn.transaction()?;
            changeset.persist_to_sqlite(&db_tx)?;
            db_tx.commit()?;
        }

        // Load from sqlite should succeed
        {
            let db_tx = conn.transaction()?;
            let changeset = ChangeSet::from_sqlite(&db_tx)?;
            db_tx.commit()?;
            assert_eq!(changeset.last_evicted.get(&txid), Some(&last_evicted));
        }

        Ok(())
    }
}
