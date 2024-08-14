use core::str::FromStr;
use std::prelude::rust_2021::{String, ToString};
use bdk_chain::{
    indexed_tx_graph, keychain_txout, local_chain, tx_graph, ConfirmationBlockTime, Merge,
};
use miniscript::{Descriptor, DescriptorPublicKey};
use chain::sqlx;
use chain::sqlx::{migrate, Row};

type IndexedTxGraphChangeSet =
    indexed_tx_graph::ChangeSet<ConfirmationBlockTime, keychain_txout::ChangeSet>;

/// A changeset for [`Wallet`](crate::Wallet).
#[derive(Default, Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ChangeSet {
    /// Descriptor for recipient addresses.
    pub descriptor: Option<Descriptor<DescriptorPublicKey>>,
    /// Descriptor for change addresses.
    pub change_descriptor: Option<Descriptor<DescriptorPublicKey>>,
    /// Stores the network type of the transaction data.
    pub network: Option<bitcoin::Network>,
    /// Changes to the [`LocalChain`](local_chain::LocalChain).
    pub local_chain: local_chain::ChangeSet,
    /// Changes to [`TxGraph`](tx_graph::TxGraph).
    pub tx_graph: tx_graph::ChangeSet<ConfirmationBlockTime>,
    /// Changes to [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex).
    pub indexer: keychain_txout::ChangeSet,
}

impl Merge for ChangeSet {
    /// Merge another [`ChangeSet`] into itself.
    fn merge(&mut self, other: Self) {
        if other.descriptor.is_some() {
            debug_assert!(
                self.descriptor.is_none() || self.descriptor == other.descriptor,
                "descriptor must never change"
            );
            self.descriptor = other.descriptor;
        }
        if other.change_descriptor.is_some() {
            debug_assert!(
                self.change_descriptor.is_none()
                    || self.change_descriptor == other.change_descriptor,
                "change descriptor must never change"
            );
            self.change_descriptor = other.change_descriptor;
        }
        if other.network.is_some() {
            debug_assert!(
                self.network.is_none() || self.network == other.network,
                "network must never change"
            );
            self.network = other.network;
        }

        Merge::merge(&mut self.local_chain, other.local_chain);
        Merge::merge(&mut self.tx_graph, other.tx_graph);
        Merge::merge(&mut self.indexer, other.indexer);
    }

    fn is_empty(&self) -> bool {
        self.descriptor.is_none()
            && self.change_descriptor.is_none()
            && self.network.is_none()
            && self.local_chain.is_empty()
            && self.tx_graph.is_empty()
            && self.indexer.is_empty()
    }
}

#[cfg(feature = "sqlx")]
impl ChangeSet {
    /// Schema name for wallet.
    pub const WALLET_SCHEMA_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet descriptors and network.
    pub const WALLET_TABLE_NAME: &'static str = "bdk_wallet";

    /// Initialize PostgreSQL tables for wallet schema & table.
    async fn init_wallet_postgres_tables(
        db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> sqlx::Result<()> {

        // sqlx::migrate!("/Users/matthiasdebernardini/Projects/bdk/crates/wallet/src/wallet_migrations").run(db_tx).await
        Ok(())
    }

    /// Recover a [`ChangeSet`] from PostgreSQL database.
    pub async fn from_postgres(db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> sqlx::Result<Self> {
        Self::init_wallet_postgres_tables(db_tx).await?;

        let mut changeset = Self::default();

        let row = sqlx::query(&format!(
            "SELECT descriptor, change_descriptor, network FROM {}",
            Self::WALLET_TABLE_NAME,
        ))
            .fetch_optional(&mut **db_tx)
            .await?;

        if let Some(row) = row {
            changeset.descriptor = row.get::<Option<String>, _>("descriptor")
                .and_then(|s| Descriptor::<DescriptorPublicKey>::from_str(&s).ok());
            changeset.change_descriptor = row.get::<Option<String>, _>("change_descriptor")
                .and_then(|s| Descriptor::<DescriptorPublicKey>::from_str(&s).ok());
            changeset.network = row.get::<Option<String>, _>("network")
                .and_then(|s| bitcoin::Network::from_str(&s).ok());
        }

        changeset.local_chain = local_chain::ChangeSet::from_postgres(db_tx).await?;
        changeset.tx_graph = tx_graph::ChangeSet::<_>::from_postgres(db_tx).await?;
        changeset.indexer = keychain_txout::ChangeSet::from_postgres(db_tx).await?;

        Ok(changeset)
    }

    /// Persist [`ChangeSet`] to PostgreSQL database.
    pub async fn persist_to_postgres(
        &self,
        db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> sqlx::Result<()> {
        Self::init_wallet_postgres_tables(db_tx).await?;

        if let Some(descriptor) = &self.descriptor {
            sqlx::query(&format!(
                "INSERT INTO {} (id, descriptor) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET descriptor = $2",
                Self::WALLET_TABLE_NAME,
            ))
                .bind(0)
                .bind(descriptor.to_string())
                .execute(&mut **db_tx)
                .await?;
        }

        if let Some(change_descriptor) = &self.change_descriptor {
            sqlx::query(&format!(
                "INSERT INTO {} (id, change_descriptor) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET change_descriptor = $2",
                Self::WALLET_TABLE_NAME,
            ))
                .bind(0)
                .bind(change_descriptor.to_string())
                .execute(&mut **db_tx)
                .await?;
        }

        if let Some(network) = self.network {
            sqlx::query(&format!(
                "INSERT INTO {} (id, network) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET network = $2",
                Self::WALLET_TABLE_NAME,
            ))
                .bind(0)
                .bind(network.to_string())
                .execute(&mut **db_tx)
                .await?;
        }

        self.local_chain.persist_to_postgres(db_tx).await?;
        self.tx_graph.persist_to_postgres(db_tx).await?;
        self.indexer.persist_to_postgres(db_tx).await?;
        Ok(())
    }
}


// #[cfg(feature = "rusqlite")]
// impl ChangeSet {
//     /// Schema name for wallet.
//     pub const WALLET_SCHEMA_NAME: &'static str = "bdk_wallet";
//     /// Name of table to store wallet descriptors and network.
//     pub const WALLET_TABLE_NAME: &'static str = "bdk_wallet";
//
//     /// Initialize sqlite tables for wallet schema & table.
//     fn init_wallet_sqlite_tables(
//         db_tx: &chain::rusqlite::Transaction,
//     ) -> chain::rusqlite::Result<()> {
//         let schema_v0: &[&str] = &[&format!(
//             "CREATE TABLE {} ( \
//                 id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
//                 descriptor TEXT, \
//                 change_descriptor TEXT, \
//                 network TEXT \
//                 ) STRICT;",
//             Self::WALLET_TABLE_NAME,
//         )];
//         crate::rusqlite_impl::migrate_schema(db_tx, Self::WALLET_SCHEMA_NAME, &[schema_v0])
//     }
//
//     /// Recover a [`ChangeSet`] from sqlite database.
//     pub fn from_sqlite(db_tx: &chain::rusqlite::Transaction) -> chain::rusqlite::Result<Self> {
//         Self::init_wallet_sqlite_tables(db_tx)?;
//         use chain::rusqlite::OptionalExtension;
//         use chain::Impl;
//
//         let mut changeset = Self::default();
//
//         let mut wallet_statement = db_tx.prepare(&format!(
//             "SELECT descriptor, change_descriptor, network FROM {}",
//             Self::WALLET_TABLE_NAME,
//         ))?;
//         let row = wallet_statement
//             .query_row([], |row| {
//                 Ok((
//                     row.get::<_, Impl<Descriptor<DescriptorPublicKey>>>("descriptor")?,
//                     row.get::<_, Impl<Descriptor<DescriptorPublicKey>>>("change_descriptor")?,
//                     row.get::<_, Impl<bitcoin::Network>>("network")?,
//                 ))
//             })
//             .optional()?;
//         if let Some((Impl(desc), Impl(change_desc), Impl(network))) = row {
//             changeset.descriptor = Some(desc);
//             changeset.change_descriptor = Some(change_desc);
//             changeset.network = Some(network);
//         }
//
//         changeset.local_chain = local_chain::ChangeSet::from_sqlite(db_tx)?;
//         changeset.tx_graph = tx_graph::ChangeSet::<_>::from_sqlite(db_tx)?;
//         changeset.indexer = keychain_txout::ChangeSet::from_sqlite(db_tx)?;
//
//         Ok(changeset)
//     }
//
//     /// Persist [`ChangeSet`] to sqlite database.
//     pub fn persist_to_sqlite(
//         &self,
//         db_tx: &chain::rusqlite::Transaction,
//     ) -> chain::rusqlite::Result<()> {
//         Self::init_wallet_sqlite_tables(db_tx)?;
//         use chain::rusqlite::named_params;
//         use chain::Impl;
//
//         let mut descriptor_statement = db_tx.prepare_cached(&format!(
//             "INSERT INTO {}(id, descriptor) VALUES(:id, :descriptor) ON CONFLICT(id) DO UPDATE SET descriptor=:descriptor",
//             Self::WALLET_TABLE_NAME,
//         ))?;
//         if let Some(descriptor) = &self.descriptor {
//             descriptor_statement.execute(named_params! {
//                 ":id": 0,
//                 ":descriptor": Impl(descriptor.clone()),
//             })?;
//         }
//
//         let mut change_descriptor_statement = db_tx.prepare_cached(&format!(
//             "INSERT INTO {}(id, change_descriptor) VALUES(:id, :change_descriptor) ON CONFLICT(id) DO UPDATE SET change_descriptor=:change_descriptor",
//             Self::WALLET_TABLE_NAME,
//         ))?;
//         if let Some(change_descriptor) = &self.change_descriptor {
//             change_descriptor_statement.execute(named_params! {
//                 ":id": 0,
//                 ":change_descriptor": Impl(change_descriptor.clone()),
//             })?;
//         }
//
//         let mut network_statement = db_tx.prepare_cached(&format!(
//             "INSERT INTO {}(id, network) VALUES(:id, :network) ON CONFLICT(id) DO UPDATE SET network=:network",
//             Self::WALLET_TABLE_NAME,
//         ))?;
//         if let Some(network) = self.network {
//             network_statement.execute(named_params! {
//                 ":id": 0,
//                 ":network": Impl(network),
//             })?;
//         }
//
//         self.local_chain.persist_to_sqlite(db_tx)?;
//         self.tx_graph.persist_to_sqlite(db_tx)?;
//         self.indexer.persist_to_sqlite(db_tx)?;
//         Ok(())
//     }
// }

impl From<local_chain::ChangeSet> for ChangeSet {
    fn from(chain: local_chain::ChangeSet) -> Self {
        Self {
            local_chain: chain,
            ..Default::default()
        }
    }
}

impl From<IndexedTxGraphChangeSet> for ChangeSet {
    fn from(indexed_tx_graph: IndexedTxGraphChangeSet) -> Self {
        Self {
            tx_graph: indexed_tx_graph.tx_graph,
            indexer: indexed_tx_graph.indexer,
            ..Default::default()
        }
    }
}

impl From<tx_graph::ChangeSet<ConfirmationBlockTime>> for ChangeSet {
    fn from(tx_graph: tx_graph::ChangeSet<ConfirmationBlockTime>) -> Self {
        Self {
            tx_graph,
            ..Default::default()
        }
    }
}

impl From<keychain_txout::ChangeSet> for ChangeSet {
    fn from(indexer: keychain_txout::ChangeSet) -> Self {
        Self {
            indexer,
            ..Default::default()
        }
    }
}
