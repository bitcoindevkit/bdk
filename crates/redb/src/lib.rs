//! redb storage backend for Bitcoin Devlopment Kit
//!
//! This crate provides redb based implementation of `Wallet Persister`
//! from the `bdk_wallet` crate.

use std::error::Error as StdError;
use std::fmt;
use std::path::Path;

use bdk_wallet::{ChangeSet, WalletPersister};
use bincode::{DefaultOptions, Options};
use redb::{Database, Error as RedbError, ReadableTable, TableDefinition};

// using single table with string keys for simplicity
const WALLET_TABLE: TableDefinition<str, [u8]> = TableDefinition::new("wallet_data");

// keys for different components fo changeset
const DESCRIPTOR_KEY: &str = "descriptor";
const CHANGE_DESCRIPTOR_KEY: &str = "change_descriptor";
const NETWORK_KEY: &str = "network";
const LOCAL_CHAIN_KEY: &str = "local_chain";
const TX_GRAPH_KEY: &str = "tx_graph";
const INDEXER_KEY: &str = "indexer";

/// error type for redb wallet persister
#[derive(Debug)]
pub enum RedbStoreError {
    /// redb error
    Redb(redb::Error),
    /// serialization error
    Serialization(bincode::Error),
}

impl fmt::Display for RedbStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RedbStoreError::Redb(e) => write!(f, "Redb error: {}", e),
            RedbStoreError::Serialization(e) => write!(f, "Serialization error: {}", e),
        }
    }
}

impl StdError for RedbStoreError {}

impl From<RedbError> for RedbStoreError {
    fn from(e: RedbError) -> Self {
        RedbStoreError::Redb(e)
    }
}

impl From<bincode::Error> for RedbStoreError {
    fn from(e: bincode::Error) -> Self {
        RedbStoreError::Serialization(e)
    }
}

/// redb based implementation of `WalletPersister`
pub struct RedbStore {
    db: Database,
}

fn bincode_options() -> impl bincode::Options {
    DefaultOptions::new().with_varint_encoding()
}

impl RedbStore {
    /// create new redb store at given path
    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self, RedbStoreError> {
        // Default database size (10MB should be sufficient for most wallets)
        const DEFAULT_DB_SIZE: usize = 10 * 1024 * 1024; // 10MB

        // SAFETY: We're creating a new database file. The caller is responsible for ensuring
        // the path is valid and the file can be created safely.
        let db = unsafe { Database::create(path, DEFAULT_DB_SIZE)? };

        // initialize tables
        let write_txn = db.begin_write()?;
        {
            // create wallet data table
            write_txn.open_table(WALLET_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db })
    }

    /// open existing redb store at given path
    /// As per redb 0.8.0 Database::open is unsafe
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, RedbStoreError> {
        // SAFETY: We're opening a database file that we expect to exist and be a valid redb database.
        // The caller is responsible for ensuring the path is valid and the file is not concurrently
        // modified in ways that would violate redb's assumptions.
        let db = unsafe { Database::open(path)? };
        Ok(Self { db })
    }

    /// creat a new redb store if it don;'t exist or open it if it exist
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self, RedbStoreError> {
        if path.as_ref().exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }
}

impl WalletPersister for RedbStore {
    type Error = RedbStoreError;

    fn initialize(persister: &mut Self) -> Result<ChangeSet, Self::Error> {
        // start read transaction
        let read_txn = persister.db.begin_read()?;

        // open wallet data table
        let table = read_txn.open_table(WALLET_TABLE)?;

        // create an empty changeset
        let mut changeset = ChangeSet::default();

        // load each component of the changeset
        // desc
        if let Some(value) = table.get(DESCRIPTOR_KEY)? {
            changeset.descriptor = Some(
                bincode_options()
                    .deserialize(value)
                    .map_err(RedbStoreError::Serialization)?,
            );
        }

        // change desc
        if let Some(value) = table.get(CHANGE_DESCRIPTOR_KEY)? {
            changeset.change_descriptor = Some(
                bincode_options()
                    .deserialize(value)
                    .map_err(RedbStoreError::Serialization)?,
            );
        }

        // network
        if let Some(value) = table.get(NETWORK_KEY)? {
            changeset.network = Some(
                bincode_options()
                    .deserialize(value)
                    .map_err(RedbStoreError::Serialization)?,
            );
        }

        // local chain
        if let Some(value) = table.get(LOCAL_CHAIN_KEY)? {
            changeset.local_chain = bincode_options()
                .deserialize(value)
                .map_err(RedbStoreError::Serialization)?;
        }

        // Tx graph
        if let Some(value) = table.get(TX_GRAPH_KEY)? {
            changeset.tx_graph = bincode_options()
                .deserialize(value)
                .map_err(RedbStoreError::Serialization)?;
        }

        // indxr
        if let Some(value) = table.get(INDEXER_KEY)? {
            changeset.indexer = bincode_options()
                .deserialize(value)
                .map_err(RedbStoreError::Serialization)?;
        }

        Ok(changeset)
    }

    fn persist(persister: &mut Self, changeset: &ChangeSet) -> Result<(), Self::Error> {
        // skip if the changeset is completely empty
        if changeset.descriptor.is_none()
            && changeset.change_descriptor.is_none()
            && changeset.network.is_none()
            && changeset.local_chain.blocks.is_empty()
            && changeset.tx_graph.txs.is_empty()
            && changeset.tx_graph.txouts.is_empty()
            && changeset.indexer.last_revealed.is_empty()
        {
            return Ok(());
        }

        // start write transaction
        let write_txn = persister.db.begin_write()?;

        {
            // open wallet data table
            let mut table = write_txn.open_table(WALLET_TABLE)?;

            // store each component of the changeset if it's not empty

            // desc
            if let Some(descriptor) = &changeset.descriptor {
                let serialized = bincode_options()
                    .serialize(descriptor)
                    .map_err(RedbStoreError::Serialization)?;
                table.insert(DESCRIPTOR_KEY, serialized.as_slice())?;
            }

            // change desc
            if let Some(change_descriptor) = &changeset.change_descriptor {
                let serialized = bincode_options()
                    .serialize(change_descriptor)
                    .map_err(RedbStoreError::Serialization)?;
                table.insert(CHANGE_DESCRIPTOR_KEY, serialized.as_slice())?;
            }

            // network
            if let Some(network) = &changeset.network {
                let serialized = bincode_options()
                    .serialize(network)
                    .map_err(RedbStoreError::Serialization)?;
                table.insert(NETWORK_KEY, serialized.as_slice())?;
            }

            // local chain (check if it has any blocks)
            if !changeset.local_chain.blocks.is_empty() {
                let serialized = bincode_options()
                    .serialize(&changeset.local_chain)
                    .map_err(RedbStoreError::Serialization)?;
                table.insert(LOCAL_CHAIN_KEY, serialized.as_slice())?;
            }

            // Tx graph (check if it has any tx or outputs)
            if !changeset.tx_graph.txs.is_empty()
                || !changeset.tx_graph.txouts.is_empty()
                || !changeset.tx_graph.anchors.is_empty()
                || !changeset.tx_graph.last_seen.is_empty()
            {
                let serialized = bincode_options()
                    .serialize(&changeset.tx_graph)
                    .map_err(RedbStoreError::Serialization)?;
                table.insert(TX_GRAPH_KEY, serialized.as_slice())?;
            }

            // idxr (check if it has any revealed indices)
            if !changeset.indexer.last_revealed.is_empty() {
                let serialized = bincode_options()
                    .serialize(&changeset.indexer)
                    .map_err(RedbStoreError::Serialization)?;
                table.insert(INDEXER_KEY, serialized.as_slice())?;
            }
        }

        // commit the Tx
        write_txn.commit()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Network;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_empty_store() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("wallet.redb");

        let mut store = RedbStore::create(&db_path).unwrap();

        // initialize should return an empty changeset
        let changeset = WalletPersister::initialize(&mut store).unwrap();
        assert!(changeset.descriptor.is_none());
        assert!(changeset.change_descriptor.is_none());
        assert!(changeset.network.is_none());
        assert!(changeset.local_chain.blocks.is_empty());
        assert!(changeset.tx_graph.txs.is_empty());
        assert!(changeset.indexer.last_revealed.is_empty());
    }

    #[test]
    fn test_persist_and_retrieve() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("wallet.redb");

        // create a store and persist a changeset
        {
            let mut store = RedbStore::create(&db_path).unwrap();

            // creating simple changeset with just a network value
            let mut changeset = ChangeSet::default();
            changeset.network = Some(Network::Testnet);

            // persist the changeset
            WalletPersister::persist(&mut store, &changeset).unwrap();
        }

        // open store again and check if the changeset was persisted
        {
            let mut store = RedbStore::open(&db_path).unwrap();

            // initialized should return the persisted changeset
            let changeset = WalletPersister::initialize(&mut store).unwrap();
            assert_eq!(changeset.network, Some(Network::Testnet));
        }
    }

    #[test]
    fn test_open_nonexistent_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("nonexistent.redb");

        // trying to open a non-existent file should fail
        let result = RedbStore::open(&db_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_open_or_create() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("new_wallet.redb");

        // first call should create the file
        {
            let store = RedbStore::open_or_create(&db_path);
            assert!(store.is_ok());
            assert!(Path::new(&db_path).exists());
        }

        // second call should open the existing file
        {
            let store = RedbStore::open_or_create(&db_path);
            assert!(store.is_ok());
        }
    }

    #[test]
    fn test_empty_changeset() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("wallet.redb");

        let mut store = RedbStore::create(&db_path).unwrap();

        // persisting an empty changeset should do nothing
        let empty_changeset = ChangeSet::default();
        let result = WalletPersister::persist(&mut store, &empty_changeset);
        assert!(result.is_ok());

        // initialize should still return an empty changeset
        let loaded = WalletPersister::initialize(&mut store).unwrap();
        assert!(loaded.descriptor.is_none());
        assert!(loaded.network.is_none());
    }

    #[test]
    fn test_update_existing_data() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("wallet.redb");

        // create initial data
        {
            let mut store = RedbStore::create(&db_path).unwrap();

            let mut changeset = ChangeSet::default();
            changeset.network = Some(Network::Testnet);

            WalletPersister::persist(&mut store, &changeset).unwrap();
        }

        // update with new data
        {
            let mut store = RedbStore::open(&db_path).unwrap();

            let mut changeset = ChangeSet::default();
            changeset.network = Some(Network::Bitcoin);

            WalletPersister::persist(&mut store, &changeset).unwrap();
        }

        // verify data was updated
        {
            let mut store = RedbStore::open(&db_path).unwrap();
            let loaded = WalletPersister::initialize(&mut store).unwrap();

            assert_eq!(loaded.network, Some(Network::Bitcoin));
        }
    }
}
