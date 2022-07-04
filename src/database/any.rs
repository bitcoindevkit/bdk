// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Runtime-checked database types
//!
//! This module provides the implementation of [`AnyDatabase`] which allows switching the
//! inner [`Database`] type at runtime.
//!
//! ## Example
//!
//! In this example, `wallet_memory` and `wallet_sled` have the same type of `Wallet<(), AnyDatabase>`.
//!
//! ```no_run
//! # use bitcoin::Network;
//! # use bdk::database::{AnyDatabase, MemoryDatabase};
//! # use bdk::{Wallet};
//! let memory = MemoryDatabase::default();
//! let wallet_memory = Wallet::new("...", None, Network::Testnet, memory)?;
//!
//! # #[cfg(feature = "key-value-db")]
//! # {
//! let sled = sled::open("my-database")?.open_tree("default_tree")?;
//! let wallet_sled = Wallet::new("...", None, Network::Testnet, sled)?;
//! # }
//! # Ok::<(), bdk::Error>(())
//! ```
//!
//! When paired with the use of [`ConfigurableDatabase`], it allows creating wallets with any
//! database supported using a single line of code:
//!
//! ```no_run
//! # use bitcoin::Network;
//! # use bdk::database::*;
//! # use bdk::{Wallet};
//! let config = serde_json::from_str("...")?;
//! let database = AnyDatabase::from_config(&config)?;
//! let wallet = Wallet::new("...", None, Network::Testnet, database)?;
//! # Ok::<(), bdk::Error>(())
//! ```

use super::*;

macro_rules! impl_from {
    ( $from:ty, $to:ty, $variant:ident, $( $cfg:tt )* ) => {
        $( $cfg )*
        impl From<$from> for $to {
            fn from(inner: $from) -> Self {
                <$to>::$variant(inner)
            }
        }
    };
}

macro_rules! impl_inner_method {
    ( $enum_name:ident, $self:expr, $name:ident $(, $args:expr)* ) => {
        #[allow(deprecated)]
        match $self {
            $enum_name::Memory(inner) => inner.$name( $($args, )* ),
            #[cfg(feature = "key-value-db")]
            $enum_name::Sled(inner) => inner.$name( $($args, )* ),
            #[cfg(feature = "sqlite")]
            $enum_name::Sqlite(inner) => inner.$name( $($args, )* ),
        }
    }
}

/// Type that can contain any of the [`Database`] types defined by the library
///
/// It allows switching database type at runtime.
///
/// See [this module](crate::database::any)'s documentation for a usage example.
#[derive(Debug)]
pub enum AnyDatabase {
    /// In-memory ephemeral database
    Memory(memory::MemoryDatabase),
    #[cfg(feature = "key-value-db")]
    #[cfg_attr(docsrs, doc(cfg(feature = "key-value-db")))]
    /// Simple key-value embedded database based on [`sled`]
    Sled(sled::Tree),
    #[cfg(feature = "sqlite")]
    #[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
    /// Sqlite embedded database using [`rusqlite`]
    Sqlite(sqlite::SqliteDatabase),
}

impl_from!(memory::MemoryDatabase, AnyDatabase, Memory,);
impl_from!(sled::Tree, AnyDatabase, Sled, #[cfg(feature = "key-value-db")]);
impl_from!(sqlite::SqliteDatabase, AnyDatabase, Sqlite, #[cfg(feature = "sqlite")]);

/// Type that contains any of the [`BatchDatabase::Batch`] types defined by the library
pub enum AnyBatch {
    /// In-memory ephemeral database
    Memory(<memory::MemoryDatabase as BatchDatabase>::Batch),
    #[cfg(feature = "key-value-db")]
    #[cfg_attr(docsrs, doc(cfg(feature = "key-value-db")))]
    /// Simple key-value embedded database based on [`sled`]
    Sled(<sled::Tree as BatchDatabase>::Batch),
    #[cfg(feature = "sqlite")]
    #[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
    /// Sqlite embedded database using [`rusqlite`]
    Sqlite(<sqlite::SqliteDatabase as BatchDatabase>::Batch),
}

impl_from!(
    <memory::MemoryDatabase as BatchDatabase>::Batch,
    AnyBatch,
    Memory,
);
impl_from!(<sled::Tree as BatchDatabase>::Batch, AnyBatch, Sled, #[cfg(feature = "key-value-db")]);
impl_from!(<sqlite::SqliteDatabase as BatchDatabase>::Batch, AnyBatch, Sqlite, #[cfg(feature = "sqlite")]);

impl BatchOperations for AnyDatabase {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<(), Error> {
        impl_inner_method!(
            AnyDatabase,
            self,
            set_script_pubkey,
            script,
            keychain,
            child
        )
    }
    fn set_utxo(&mut self, utxo: &LocalUtxo) -> Result<(), Error> {
        impl_inner_method!(AnyDatabase, self, set_utxo, utxo)
    }
    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
        impl_inner_method!(AnyDatabase, self, set_raw_tx, transaction)
    }
    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error> {
        impl_inner_method!(AnyDatabase, self, set_tx, transaction)
    }
    fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), Error> {
        impl_inner_method!(AnyDatabase, self, set_last_index, keychain, value)
    }
    fn set_sync_time(&mut self, sync_time: SyncTime) -> Result<(), Error> {
        impl_inner_method!(AnyDatabase, self, set_sync_time, sync_time)
    }

    fn del_script_pubkey_from_path(
        &mut self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<Script>, Error> {
        impl_inner_method!(
            AnyDatabase,
            self,
            del_script_pubkey_from_path,
            keychain,
            child
        )
    }
    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        impl_inner_method!(AnyDatabase, self, del_path_from_script_pubkey, script)
    }
    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        impl_inner_method!(AnyDatabase, self, del_utxo, outpoint)
    }
    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        impl_inner_method!(AnyDatabase, self, del_raw_tx, txid)
    }
    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, Error> {
        impl_inner_method!(AnyDatabase, self, del_tx, txid, include_raw)
    }
    fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        impl_inner_method!(AnyDatabase, self, del_last_index, keychain)
    }
    fn del_sync_time(&mut self) -> Result<Option<SyncTime>, Error> {
        impl_inner_method!(AnyDatabase, self, del_sync_time)
    }
}

impl Database for AnyDatabase {
    fn check_descriptor_checksum<B: AsRef<[u8]>>(
        &mut self,
        keychain: KeychainKind,
        bytes: B,
    ) -> Result<(), Error> {
        impl_inner_method!(
            AnyDatabase,
            self,
            check_descriptor_checksum,
            keychain,
            bytes
        )
    }

    fn iter_script_pubkeys(&self, keychain: Option<KeychainKind>) -> Result<Vec<Script>, Error> {
        impl_inner_method!(AnyDatabase, self, iter_script_pubkeys, keychain)
    }
    fn iter_utxos(&self) -> Result<Vec<LocalUtxo>, Error> {
        impl_inner_method!(AnyDatabase, self, iter_utxos)
    }
    fn iter_raw_txs(&self) -> Result<Vec<Transaction>, Error> {
        impl_inner_method!(AnyDatabase, self, iter_raw_txs)
    }
    fn iter_txs(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        impl_inner_method!(AnyDatabase, self, iter_txs, include_raw)
    }

    fn get_script_pubkey_from_path(
        &self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<Script>, Error> {
        impl_inner_method!(
            AnyDatabase,
            self,
            get_script_pubkey_from_path,
            keychain,
            child
        )
    }
    fn get_path_from_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        impl_inner_method!(AnyDatabase, self, get_path_from_script_pubkey, script)
    }
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        impl_inner_method!(AnyDatabase, self, get_utxo, outpoint)
    }
    fn get_raw_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        impl_inner_method!(AnyDatabase, self, get_raw_tx, txid)
    }
    fn get_tx(&self, txid: &Txid, include_raw: bool) -> Result<Option<TransactionDetails>, Error> {
        impl_inner_method!(AnyDatabase, self, get_tx, txid, include_raw)
    }
    fn get_last_index(&self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        impl_inner_method!(AnyDatabase, self, get_last_index, keychain)
    }
    fn get_sync_time(&self) -> Result<Option<SyncTime>, Error> {
        impl_inner_method!(AnyDatabase, self, get_sync_time)
    }

    fn increment_last_index(&mut self, keychain: KeychainKind) -> Result<u32, Error> {
        impl_inner_method!(AnyDatabase, self, increment_last_index, keychain)
    }
}

impl BatchOperations for AnyBatch {
    fn set_script_pubkey(
        &mut self,
        script: &Script,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<(), Error> {
        impl_inner_method!(AnyBatch, self, set_script_pubkey, script, keychain, child)
    }
    fn set_utxo(&mut self, utxo: &LocalUtxo) -> Result<(), Error> {
        impl_inner_method!(AnyBatch, self, set_utxo, utxo)
    }
    fn set_raw_tx(&mut self, transaction: &Transaction) -> Result<(), Error> {
        impl_inner_method!(AnyBatch, self, set_raw_tx, transaction)
    }
    fn set_tx(&mut self, transaction: &TransactionDetails) -> Result<(), Error> {
        impl_inner_method!(AnyBatch, self, set_tx, transaction)
    }
    fn set_last_index(&mut self, keychain: KeychainKind, value: u32) -> Result<(), Error> {
        impl_inner_method!(AnyBatch, self, set_last_index, keychain, value)
    }
    fn set_sync_time(&mut self, sync_time: SyncTime) -> Result<(), Error> {
        impl_inner_method!(AnyBatch, self, set_sync_time, sync_time)
    }

    fn del_script_pubkey_from_path(
        &mut self,
        keychain: KeychainKind,
        child: u32,
    ) -> Result<Option<Script>, Error> {
        impl_inner_method!(AnyBatch, self, del_script_pubkey_from_path, keychain, child)
    }
    fn del_path_from_script_pubkey(
        &mut self,
        script: &Script,
    ) -> Result<Option<(KeychainKind, u32)>, Error> {
        impl_inner_method!(AnyBatch, self, del_path_from_script_pubkey, script)
    }
    fn del_utxo(&mut self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        impl_inner_method!(AnyBatch, self, del_utxo, outpoint)
    }
    fn del_raw_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        impl_inner_method!(AnyBatch, self, del_raw_tx, txid)
    }
    fn del_tx(
        &mut self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, Error> {
        impl_inner_method!(AnyBatch, self, del_tx, txid, include_raw)
    }
    fn del_last_index(&mut self, keychain: KeychainKind) -> Result<Option<u32>, Error> {
        impl_inner_method!(AnyBatch, self, del_last_index, keychain)
    }
    fn del_sync_time(&mut self) -> Result<Option<SyncTime>, Error> {
        impl_inner_method!(AnyBatch, self, del_sync_time)
    }
}

impl BatchDatabase for AnyDatabase {
    type Batch = AnyBatch;

    fn begin_batch(&self) -> Self::Batch {
        match self {
            AnyDatabase::Memory(inner) => inner.begin_batch().into(),
            #[cfg(feature = "key-value-db")]
            AnyDatabase::Sled(inner) => inner.begin_batch().into(),
            #[cfg(feature = "sqlite")]
            AnyDatabase::Sqlite(inner) => inner.begin_batch().into(),
        }
    }
    fn commit_batch(&mut self, batch: Self::Batch) -> Result<(), Error> {
        match self {
            AnyDatabase::Memory(db) => match batch {
                AnyBatch::Memory(batch) => db.commit_batch(batch),
                #[cfg(any(feature = "key-value-db", feature = "sqlite"))]
                _ => unimplemented!("Other batch shouldn't be used with Memory db."),
            },
            #[cfg(feature = "key-value-db")]
            AnyDatabase::Sled(db) => match batch {
                AnyBatch::Sled(batch) => db.commit_batch(batch),
                _ => unimplemented!("Other batch shouldn't be used with Sled db."),
            },
            #[cfg(feature = "sqlite")]
            AnyDatabase::Sqlite(db) => match batch {
                AnyBatch::Sqlite(batch) => db.commit_batch(batch),
                _ => unimplemented!("Other batch shouldn't be used with Sqlite db."),
            },
        }
    }
}

/// Configuration type for a [`sled::Tree`] database
#[cfg(feature = "key-value-db")]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SledDbConfiguration {
    /// Main directory of the db
    pub path: String,
    /// Name of the database tree, a separated namespace for the data
    pub tree_name: String,
}

#[cfg(feature = "key-value-db")]
impl ConfigurableDatabase for sled::Tree {
    type Config = SledDbConfiguration;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        Ok(sled::open(&config.path)?.open_tree(&config.tree_name)?)
    }
}

/// Configuration type for a [`sqlite::SqliteDatabase`] database
#[cfg(feature = "sqlite")]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SqliteDbConfiguration {
    /// Main directory of the db
    pub path: String,
}

#[cfg(feature = "sqlite")]
impl ConfigurableDatabase for sqlite::SqliteDatabase {
    type Config = SqliteDbConfiguration;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        Ok(sqlite::SqliteDatabase::new(config.path.clone()))
    }
}

/// Type that can contain any of the database configurations defined by the library
///
/// This allows storing a single configuration that can be loaded into an [`AnyDatabase`]
/// instance. Wallets that plan to offer users the ability to switch blockchain backend at runtime
/// will find this particularly useful.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum AnyDatabaseConfig {
    /// Memory database has no config
    Memory(()),
    #[cfg(feature = "key-value-db")]
    #[cfg_attr(docsrs, doc(cfg(feature = "key-value-db")))]
    /// Simple key-value embedded database based on [`sled`]
    Sled(SledDbConfiguration),
    #[cfg(feature = "sqlite")]
    #[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
    /// Sqlite embedded database using [`rusqlite`]
    Sqlite(SqliteDbConfiguration),
}

impl ConfigurableDatabase for AnyDatabase {
    type Config = AnyDatabaseConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        Ok(match config {
            AnyDatabaseConfig::Memory(inner) => {
                AnyDatabase::Memory(memory::MemoryDatabase::from_config(inner)?)
            }
            #[cfg(feature = "key-value-db")]
            AnyDatabaseConfig::Sled(inner) => AnyDatabase::Sled(sled::Tree::from_config(inner)?),
            #[cfg(feature = "sqlite")]
            AnyDatabaseConfig::Sqlite(inner) => {
                AnyDatabase::Sqlite(sqlite::SqliteDatabase::from_config(inner)?)
            }
        })
    }
}

impl_from!((), AnyDatabaseConfig, Memory,);
impl_from!(SledDbConfiguration, AnyDatabaseConfig, Sled, #[cfg(feature = "key-value-db")]);
impl_from!(SqliteDbConfiguration, AnyDatabaseConfig, Sqlite, #[cfg(feature = "sqlite")]);
