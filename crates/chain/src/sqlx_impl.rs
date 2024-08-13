//! Module for stuff

use crate::*;
use core::str::FromStr;

use alloc::{string::ToString, sync::Arc, vec::Vec};
use std::prelude::rust_2021::String;
use bitcoin::consensus::{Decodable, Encodable};
// use rusqlite;
use sqlx;
use sqlx::{Acquire, migrate, Postgres, Row};
use sqlx::migrate::MigrateError;
use sqlx::postgres::PgRow;
// use rusqlite::named_params;
// use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
// use rusqlite::OptionalExtension;
// use rusqlite::Transaction;

/// Table name for schemas.
pub const SCHEMAS_TABLE_NAME: &str = "bdk_schemas";


impl<A> tx_graph::ChangeSet<A>
where
    A: Anchor + Clone + Ord + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Schema name for [`tx_graph::ChangeSet`].
    pub const SCHEMA_NAME: &'static str = "bdk_txgraph";
    /// Name of table that stores full transactions and `last_seen` timestamps.
    pub const TXS_TABLE_NAME: &'static str = "bdk_txs";
    /// Name of table that stores floating txouts.
    pub const TXOUTS_TABLE_NAME: &'static str = "bdk_txouts";
    /// Name of table that stores [`Anchor`]s.
    pub const ANCHORS_TABLE_NAME: &'static str = "bdk_anchors";

    /// Initialize sqlite tables.
    async fn init_postgres_tables(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), MigrateError> {
        // let schema_v0: &[&str] = &[
        //     // full transactions
        //     &format!(
        //         "CREATE TABLE IF NOT EXISTS {} ( \
        // txid TEXT PRIMARY KEY NOT NULL, \
        // raw_tx BYTEA, \
        // last_seen BIGINT \
        // )",
        //         Self::TXS_TABLE_NAME,
        //     ),
        //     // floating txouts
        //     &format!(
        //         "CREATE TABLE IF NOT EXISTS {} ( \
        // txid TEXT NOT NULL, \
        // vout INTEGER NOT NULL, \
        // value BIGINT NOT NULL, \
        // script BYTEA NOT NULL, \
        // PRIMARY KEY (txid, vout) \
        // )",
        //         Self::TXOUTS_TABLE_NAME,
        //     ),
        //     // anchors
        //     &format!(
        //         "CREATE TABLE IF NOT EXISTS {} ( \
        // txid TEXT NOT NULL REFERENCES {} (txid), \
        // block_height INTEGER NOT NULL, \
        // block_hash TEXT NOT NULL, \
        // anchor JSONB NOT NULL, \
        // PRIMARY KEY (txid, block_height, block_hash) \
        // )",
        //         Self::ANCHORS_TABLE_NAME,
        //         Self::TXS_TABLE_NAME,
        //     ),
        // ];
        //
        // // migrate_schema(db_tx, Self::SCHEMA_NAME, &[schema_v0])
        // Ok(())
        // let db = db_tx.acquire().await.unwrap();
        migrate!("./tx_graph_migrations").run(db_tx).await


    }

    pub async fn from_postgres(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<Self> {
        Self::init_postgres_tables(db_tx).await?;

        let mut changeset = Self::default();

        let rows: Vec<PgRow> = sqlx::query(&format!(
            "SELECT txid, raw_tx, last_seen FROM {}",
            Self::TXS_TABLE_NAME,
        ))
            .fetch_all(&mut **db_tx)
            .await?;

        for row in rows {
            // let txid: bitcoin::Txid = roow.get("txid");
            let txid_str: String = row.get("txid");
            let txid = bitcoin::Txid::from_str(&txid_str).expect("Invalid txid");            let raw_tx: Option<Vec<u8>> = row.get("raw_tx");
            let last_seen: Option<i64> = row.get("last_seen");

            if let Some(tx_bytes) = raw_tx {
                if let Ok(tx) = bitcoin::Transaction::consensus_decode(&mut tx_bytes.as_slice()) {
                    changeset.txs.insert(Arc::new(tx));
                }
            }
            if let Some(last_seen) = last_seen {
                changeset.last_seen.insert(txid, last_seen as u64);
            }
        }

        let rows: Vec<PgRow> = sqlx::query(&format!(
            "SELECT txid, vout, value, script FROM {}",
            Self::TXOUTS_TABLE_NAME,
        ))
            .fetch_all(&mut **db_tx)
            .await?;

        for row in rows {
            // let txid: bitcoin::Txid = roow.get("txid");
            let txid_str: String = row.get("txid");
            let txid = bitcoin::Txid::from_str(&txid_str).expect("Invalid txid");
            let vout: i32 = row.get("vout");
            let value: i64 = row.get("value");
            let script: Vec<u8> = row.get("script");

            changeset.txouts.insert(
                bitcoin::OutPoint { txid, vout: vout as u32 },
                bitcoin::TxOut {
                    value: bitcoin::Amount::from_sat(value as u64),
                    script_pubkey: bitcoin::ScriptBuf::from(script),
                },
            );
        }

        let rows: Vec<PgRow>= sqlx::query(&format!(
            "SELECT anchor, txid FROM {}",
            Self::ANCHORS_TABLE_NAME,
        ))
            .fetch_all(&mut **db_tx)
            .await?;

        for row in rows {
            let anchor: serde_json::Value = row.get("anchor");
            // let txid: bitcoin::Txid = roow.get("txid");
            let txid_str: String = row.get("txid");
            let txid = bitcoin::Txid::from_str(&txid_str).expect("Invalid txid");

            if let Ok(anchor) = serde_json::from_value::<A>(anchor) {
                changeset.anchors.insert((anchor, txid));
            }
        }

        Ok(changeset)
    }

    pub async fn persist_to_postgres(&self, db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<()> {
        Self::init_postgres_tables(db_tx).await?;

        for tx in &self.txs {
            sqlx::query(&format!(
                "INSERT INTO {} (txid, raw_tx) VALUES ($1, $2) ON CONFLICT (txid) DO UPDATE SET raw_tx = $2",
                Self::TXS_TABLE_NAME,
            ))
                .bind(tx.compute_txid().to_string())
                .bind(bitcoin::consensus::serialize(tx.as_ref()))
                .execute(&mut **db_tx)
                .await?;
        }

        for (&txid, &last_seen) in &self.last_seen {
            sqlx::query(&format!(
                "INSERT INTO {} (txid, last_seen) VALUES ($1, $2) ON CONFLICT (txid) DO UPDATE SET last_seen = $2",
                Self::TXS_TABLE_NAME,
            ))
                .bind(txid.to_string())
                .bind(last_seen as i64)
                .execute(&mut **db_tx)
                .await?;
        }

        for (op, txo) in &self.txouts {
            sqlx::query(&format!(
                "INSERT INTO {} (txid, vout, value, script) VALUES ($1, $2, $3, $4) ON CONFLICT (txid, vout) DO UPDATE SET value = $3, script = $4",
                Self::TXOUTS_TABLE_NAME,
            ))
                .bind(op.txid.to_string())
                .bind(op.vout as i32)
                .bind(txo.value.to_sat() as i64)
                .bind(txo.script_pubkey.as_bytes())
                .execute(&mut **db_tx)
                .await?;
        }

        for (anchor, txid) in &self.anchors {
            let anchor_block = anchor.anchor_block();
            sqlx::query(&format!(
                "INSERT INTO {} (txid, block_height, block_hash, anchor) VALUES ($1, $2, $3, $4) ON CONFLICT (txid, block_height, block_hash) DO UPDATE SET anchor = $4",
                Self::ANCHORS_TABLE_NAME,
            ))
                .bind(txid.to_string())
                .bind(anchor_block.height as i32)
                .bind(anchor_block.hash.to_string())
                .bind(serde_json::to_value(anchor).unwrap())
                .execute(&mut **db_tx)
                .await?;
        }

        Ok(())
    }
}

impl local_chain::ChangeSet {
    /// Schema name for the changeset.
    pub const SCHEMA_NAME: &'static str = "bdk_localchain";
    /// Name of sqlite table that stores blocks of [`LocalChain`](local_chain::LocalChain).
    pub const BLOCKS_TABLE_NAME: &'static str = "bdk_blocks";

    /// Initialize sqlite tables for persisting [`local_chain::LocalChain`].
    async fn init_postgres_tables(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), MigrateError> {
        sqlx::migrate!("./local_chain_migrations").run(&mut **db_tx).await
    }

    pub async fn from_postgres(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<Self> {
        Self::init_postgres_tables(db_tx).await?;

        let mut changeset = Self::default();

        let rows = sqlx::query(&format!(
            "SELECT block_height, block_hash FROM {}",
            Self::BLOCKS_TABLE_NAME,
        ))
            .fetch_all(&mut **db_tx)
            .await?;

        for row in rows {
            let height: i32 = row.get("block_height");
            let hash: String = row.get("block_hash");
            if let Ok(block_hash) = bitcoin::BlockHash::from_str(&hash) {
                changeset.blocks.insert(height as u32, Some(block_hash));
            }
        }

        Ok(changeset)
    }

    pub async fn persist_to_postgres(&self, db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<()> {
        Self::init_postgres_tables(db_tx).await?;

        for (&height, &hash) in &self.blocks {
            match hash {
                Some(hash) => {
                    sqlx::query(&format!(
                        "INSERT INTO {} (block_height, block_hash) VALUES ($1, $2) ON CONFLICT (block_height) DO UPDATE SET block_hash = $2",
                        Self::BLOCKS_TABLE_NAME,
                    ))
                        .bind(height as i32)
                        .bind(hash.to_string())
                        .execute(&mut **db_tx)
                        .await?;
                },
                None => {
                    sqlx::query(&format!(
                        "DELETE FROM {} WHERE block_height = $1",
                        Self::BLOCKS_TABLE_NAME,
                    ))
                        .bind(height as i32)
                        .execute(&mut **db_tx)
                        .await?;
                },
            }
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

    /// Initialize PostgreSQL tables for persisting
    /// [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex).
    async fn init_postgres_tables(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<(), MigrateError> {
        sqlx::migrate!("./keychain_migrations").run(&mut **db_tx).await

    }

    /// Construct [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex) from PostgreSQL database
    /// and given parameters.
    pub async fn from_postgres(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<Self> {
        Self::init_postgres_tables(db_tx).await?;

        let mut changeset = Self::default();

        let rows = sqlx::query(&format!(
            "SELECT descriptor_id, last_revealed FROM {}",
            Self::LAST_REVEALED_TABLE_NAME,
        ))
            .fetch_all(&mut **db_tx)
            .await?;

        for row in rows {
            let descriptor_id: String = row.get("descriptor_id");
            let last_revealed: i64 = row.get("last_revealed");

            if let Ok(descriptor_id) = DescriptorId::from_str(&descriptor_id) {
                changeset.last_revealed.insert(descriptor_id, last_revealed as u32);
            }
        }

        Ok(changeset)
    }
    /// Persist `changeset` to the PostgreSQL database.
    pub async fn persist_to_postgres(&self, db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<()> {
        Self::init_postgres_tables(db_tx).await?;

        for (&descriptor_id, &last_revealed) in &self.last_revealed {
            sqlx::query(&format!(
                "INSERT INTO {} (descriptor_id, last_revealed) VALUES ($1, $2) ON CONFLICT (descriptor_id) DO UPDATE SET last_revealed = $2",
                Self::LAST_REVEALED_TABLE_NAME,
            ))
                .bind(descriptor_id.to_string())
                .bind(last_revealed as i64)
                .execute(&mut **db_tx)
                .await?;
        }

        Ok(())
    }
}
