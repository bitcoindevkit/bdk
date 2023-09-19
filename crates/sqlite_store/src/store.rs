use log::info;
use rusqlite::{named_params, Connection, Error};
use std::marker::PhantomData;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fmt::Debug, path::Path};

use bdk_chain::{Append, PersistBackend};

use crate::{AppendError, IterError};

const MIGRATIONS: &[&str] = &[
    // schema version control
    "CREATE TABLE version (version INTEGER)",
    "INSERT INTO version VALUES (1)",
    // changeset data
    "CREATE TABLE changeset (timestamp INTEGER NOT NULL, json TEXT NOT NULL);",
];

/// Persists changesets (`C`) to a single SQLite DB file.
///
/// The changesets are the results of altering wallet blockchain data.
#[derive(Debug)]
pub struct Store<C> {
    /// A rusqlite connection object to the SQLite database
    pub conn: Connection,
    marker: PhantomData<C>,
}

impl<C> PersistBackend<C> for Store<C>
where
    C: Default + Append + serde::Serialize + serde::de::DeserializeOwned,
{
    type WriteError = AppendError;

    type LoadError = IterError;

    fn write_changes(&mut self, changeset: &C) -> Result<(), Self::WriteError> {
        self.append_changeset(changeset)
    }

    fn load_from_persistence(&mut self) -> Result<C, Self::LoadError> {
        self.aggregate_changesets()
    }
}

impl<C> Store<C>
where
    C: Default + Append + serde::Serialize + serde::de::DeserializeOwned,
{
    /// Creates a new store from a [`Path`].
    ///
    /// The file must have been opened with read and write permissions.
    ///
    /// [`Path`]: std::path::Path
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let mut conn = Connection::open(path)?;
        Self::migrate(&mut conn)?;

        Ok(Self {
            conn,
            marker: Default::default(),
        })
    }

    /// Creates a new memory db store.
    pub fn new_memory() -> Result<Self, Error> {
        let mut conn = Connection::open_in_memory()?;
        Self::migrate(&mut conn)?;

        Ok(Self {
            conn,
            marker: Default::default(),
        })
    }

    pub fn migrate(conn: &mut Connection) -> Result<(), Error> {
        let version = Self::get_schema_version(conn)?;
        let stmts = &MIGRATIONS[(version as usize)..];

        // begin transaction, all migration statements and new schema version commit or rollback
        let tx = conn.transaction()?;

        // execute every statement and return `Some` new schema version
        // if execution fails, return `Error::Rusqlite`
        // if no statements executed returns `None`
        let new_version = stmts
            .iter()
            .enumerate()
            .map(|version_stmt| {
                info!(
                    "executing db migration {}: `{}`",
                    version + version_stmt.0 as i32 + 1,
                    version_stmt.1
                );
                tx.execute(version_stmt.1, [])
                    // map result value to next migration version
                    .map(|_| version_stmt.0 as i32 + version + 1)
            })
            .last()
            .transpose()?;

        // if `Some` new statement version, set new schema version
        if let Some(version) = new_version {
            Self::set_schema_version(&tx, version)?;
        } else {
            info!("db up to date, no migration needed");
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

    /// Loads all the changesets that have been stored as one giant changeset.
    ///
    /// This function returns a tuple of the aggregate changeset and a result that indicates
    /// whether an error occurred while reading or deserializing one of the entries. If so the
    /// changeset will consist of all of those it was able to read.
    ///
    /// You should usually check the error. In many applications, it may make sense to do a full
    /// wallet scan with a stop-gap after getting an error, since it is likely that one of the
    /// changesets it was unable to read changed the derivation indices of the tracker.
    ///
    /// **WARNING**: This method changes the write position of the underlying file. The next
    /// changeset will be written over the erroring entry (or the end of the file if none existed).
    pub fn aggregate_changesets(&mut self) -> Result<C, IterError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT rowid, json FROM changeset")
            .expect("select changesets statement");

        let rows = stmt
            .query_map([], |row| {
                let rowid = row.get_unwrap::<usize, usize>(0);
                row.get_ref(1).and_then(|r| Ok(r.as_str()?)).map(|j| {
                    serde_json::from_str::<C>(j).map_err(|e| IterError::Json { rowid, err: e })
                })
            })
            .map_err(IterError::Sqlite)?;

        let result = rows
            .into_iter()
            .try_fold(C::default(), |mut aggregate, row_changeset| {
                let changeset_result = row_changeset.map_err(IterError::Sqlite);
                match changeset_result {
                    Ok(Ok(changeset)) => {
                        aggregate.append(changeset);
                        Ok(aggregate)
                    }
                    Ok(Err(e)) => Err((aggregate, e)),
                    Err(e) => Err((aggregate, e)),
                }
            });

        match result {
            Ok(changeset) => Ok(changeset),
            Err((changeset, IterError::Json { rowid, err: _ })) => {
                // remove bad changesets starting with first errored rowid
                let mut stmt = self
                    .conn
                    .prepare("DELETE FROM changeset where rowid >= :rowid")
                    .expect("delete changeset statement");

                stmt.execute(named_params! {":rowid": rowid })
                    .map_err(IterError::Sqlite)?;
                Ok(changeset)
            }
            Err((_, err)) => Err(err),
        }
    }

    /// Append a new changeset to the file and truncate the file to the end of the appended
    /// changeset.
    ///
    /// The truncation is to avoid the possibility of having a valid but inconsistent changeset
    /// directly after the appended changeset.
    pub fn append_changeset(&mut self, changeset: &C) -> Result<(), AppendError> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        let conn = &self.conn;
        let json = serde_json::to_string(changeset)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current timestamp")
            .as_secs();

        let mut stmt = conn
            .prepare_cached("INSERT INTO changeset (timestamp, json) VALUES (:timestamp, :json)")
            .expect("insert changeset statement");
        let rows = stmt
            .execute(named_params! {":timestamp": timestamp, ":json": json })
            .map_err(AppendError::Sqlite)?;
        assert_eq!(rows, 1);

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    type TestChangeSet = Vec<String>;

    #[derive(Debug)]
    struct TestTracker;

    #[test]
    fn insert_and_load_aggregate_changesets() {
        // initial data to write to file (magic bytes + invalid data)
        let changeset1 = vec!["one".into()];
        let changeset2 = vec!["two".into()];
        let changeset3 = vec!["three!".into()];

        let mut store = Store::<TestChangeSet>::new_memory().expect("create new memory db store");
        // let mut store = Store::<TestChangeSet>::new(Path::new("test_agg.db"))
        //     .expect("create new file db store");

        store
            .append_changeset(&changeset1)
            .expect("append changeset1");
        store
            .append_changeset(&changeset2)
            .expect("append changeset2");
        store
            .append_changeset(&changeset3)
            .expect("append changeset3");

        let agg_changeset = store.aggregate_changesets().expect("aggregated changeset");

        assert_eq!(agg_changeset, vec!("one", "two", "three!")); // TODO assert
    }

    #[test]
    fn remove_bad_rows_on_load_aggregate_changesets() {
        // initial data to write to file (magic bytes + invalid data)
        let changeset1: Vec<String> = vec!["one".into()];
        let changeset2: Vec<String> = vec!["two".into()];

        let mut store = Store::<TestChangeSet>::new_memory().expect("create new memory db store");
        // let mut store = Store::<TestChangeSet>::new(Path::new("test_bad.db"))
        //     .expect("create new file db store");

        store.append_changeset(&changeset1).expect("appended");
        store.append_changeset(&changeset2).expect("appended");

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current timestamp")
            .as_secs();

        let bad_json1 = "bad1!";
        let rows = store
            .conn
            .execute(
                "INSERT INTO changeset (timestamp, json) VALUES (:timestamp, :json)",
                named_params! {":timestamp": timestamp, ":json": bad_json1 },
            )
            .expect("insert");
        assert_eq!(rows, 1);

        let bad_json2 = "bad2!!";
        let rows = store
            .conn
            .execute(
                "INSERT INTO changeset (timestamp, json) VALUES (:timestamp, :json)",
                named_params! {":timestamp": timestamp, ":json": bad_json2 },
            )
            .expect("insert");
        assert_eq!(rows, 1);

        let changeset5: Vec<String> = vec!["five".into()];
        store.append_changeset(&changeset5).expect("appended");

        let agg_changeset = store.aggregate_changesets().expect("aggregated changeset");

        assert_eq!(agg_changeset, vec!("one", "two"));

        let rows: usize = store
            .conn
            .query_row("SELECT count(*) FROM changeset", [], |row| row.get(0))
            .expect("number of rows");
        assert_eq!(rows, 2);
    }
}
