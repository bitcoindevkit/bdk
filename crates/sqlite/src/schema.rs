use crate::Store;
use rusqlite::{named_params, Connection, Error};

const SCHEMA_0: &str = include_str!("../schema/schema_0.sql");
const MIGRATIONS: &[&str] = &[SCHEMA_0];

/// Schema migration related functions.
impl<K, A> Store<K, A> {
    /// Migrate sqlite db schema to latest version.
    pub(crate) fn migrate(conn: &mut Connection) -> Result<(), Error> {
        let stmts = &MIGRATIONS
            .iter()
            .flat_map(|stmt| {
                // remove comment lines
                let s = stmt
                    .split('\n')
                    .filter(|l| !l.starts_with("--") && !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                // split into statements
                s.split(';')
                    // remove extra spaces
                    .map(|s| {
                        s.trim()
                            .split(' ')
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect::<Vec<_>>()
            })
            // remove empty statements
            .filter(|s| !s.is_empty())
            .collect::<Vec<String>>();

        let version = Self::get_schema_version(conn)?;
        let stmts = &stmts[(version as usize)..];

        // begin transaction, all migration statements and new schema version commit or rollback
        let tx = conn.transaction()?;

        // execute every statement and return `Some` new schema version
        // if execution fails, return `Error::Rusqlite`
        // if no statements executed returns `None`
        let new_version = stmts
            .iter()
            .enumerate()
            .map(|version_stmt| {
                tx.execute(version_stmt.1.as_str(), [])
                    // map result value to next migration version
                    .map(|_| version_stmt.0 as i32 + version + 1)
            })
            .last()
            .transpose()?;

        // if `Some` new statement version, set new schema version
        if let Some(version) = new_version {
            Self::set_schema_version(&tx, version)?;
        }

        // commit transaction
        tx.commit()?;
        Ok(())
    }

    fn get_schema_version(conn: &Connection) -> rusqlite::Result<i32> {
        let statement = conn.prepare_cached("SELECT version FROM version");
        match statement {
            Err(Error::SqliteFailure(e, Some(msg))) => {
                if msg == "no such table: version" {
                    Ok(0)
                } else {
                    Err(Error::SqliteFailure(e, Some(msg)))
                }
            }
            Ok(mut stmt) => {
                let mut rows = stmt.query([])?;
                match rows.next()? {
                    Some(row) => {
                        let version: i32 = row.get(0)?;
                        Ok(version)
                    }
                    None => Ok(0),
                }
            }
            _ => Ok(0),
        }
    }

    fn set_schema_version(conn: &Connection, version: i32) -> rusqlite::Result<usize> {
        conn.execute(
            "UPDATE version SET version=:version",
            named_params! {":version": version},
        )
    }
}
