//! Module for stuff

use core::{ops::Deref, str::FromStr};

use alloc::{borrow::ToOwned, boxed::Box, string::ToString, vec::Vec};
use bitcoin::consensus::{Decodable, Encodable};
pub use rusqlite;
pub use rusqlite::Connection;
use rusqlite::OptionalExtension;
pub use rusqlite::Transaction;
use rusqlite::{
    named_params,
    types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef},
    ToSql,
};

use crate::Anchor;

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
    versioned_scripts: &[&[&str]],
) -> rusqlite::Result<()> {
    init_schemas_table(db_tx)?;
    let current_version = schema_version(db_tx, schema_name)?;
    let exec_from = current_version.map_or(0_usize, |v| v as usize + 1);
    let scripts_to_exec = versioned_scripts.iter().enumerate().skip(exec_from);
    for (version, &script) in scripts_to_exec {
        set_schema_version(db_tx, schema_name, version as u32)?;
        for statement in script {
            db_tx.execute(statement, ())?;
        }
    }
    Ok(())
}

/// A wrapper so that we can impl [FromSql] and [ToSql] for multiple types.
pub struct Sql<T>(pub T);

impl<T> From<T> for Sql<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for Sql<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromSql for Sql<bitcoin::Txid> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::Txid::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Sql<bitcoin::Txid> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromSql for Sql<bitcoin::BlockHash> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::BlockHash::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Sql<bitcoin::BlockHash> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

#[cfg(feature = "miniscript")]
impl FromSql for Sql<crate::DescriptorId> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        crate::DescriptorId::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

#[cfg(feature = "miniscript")]
impl ToSql for Sql<crate::DescriptorId> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromSql for Sql<bitcoin::Transaction> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::Transaction::consensus_decode_from_finite_reader(&mut value.as_bytes()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Sql<bitcoin::Transaction> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let mut bytes = Vec::<u8>::new();
        self.consensus_encode(&mut bytes).map_err(to_sql_error)?;
        Ok(bytes.into())
    }
}

impl FromSql for Sql<bitcoin::ScriptBuf> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(bitcoin::Script::from_bytes(value.as_bytes()?)
            .to_owned()
            .into())
    }
}

impl ToSql for Sql<bitcoin::ScriptBuf> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_bytes().into())
    }
}

impl FromSql for Sql<bitcoin::Amount> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(bitcoin::Amount::from_sat(value.as_i64()?.try_into().map_err(from_sql_error)?).into())
    }
}

impl ToSql for Sql<bitcoin::Amount> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let amount: i64 = self.to_sat().try_into().map_err(to_sql_error)?;
        Ok(amount.into())
    }
}

impl<A: Anchor + serde_crate::de::DeserializeOwned> FromSql for Sql<A> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        serde_json::from_str(value.as_str()?)
            .map(Sql)
            .map_err(from_sql_error)
    }
}

impl<A: Anchor + serde_crate::Serialize> ToSql for Sql<A> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        serde_json::to_string(&self.0)
            .map(Into::into)
            .map_err(to_sql_error)
    }
}

#[cfg(feature = "miniscript")]
impl FromSql for Sql<miniscript::Descriptor<miniscript::DescriptorPublicKey>> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        miniscript::Descriptor::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

#[cfg(feature = "miniscript")]
impl ToSql for Sql<miniscript::Descriptor<miniscript::DescriptorPublicKey>> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.to_string().into())
    }
}

impl FromSql for Sql<bitcoin::Network> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        bitcoin::Network::from_str(value.as_str()?)
            .map(Self)
            .map_err(from_sql_error)
    }
}

impl ToSql for Sql<bitcoin::Network> {
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
