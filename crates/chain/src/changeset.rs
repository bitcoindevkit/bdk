use crate::{ConfirmationBlockTime, Merge};

type IndexedTxGraphChangeSet =
    crate::indexed_tx_graph::ChangeSet<ConfirmationBlockTime, crate::keychain_txout::ChangeSet>;

/// A changeset containing [`crate`] structures typically persisted together.
#[derive(Default, Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(crate::serde::Deserialize, crate::serde::Serialize),
    serde(crate = "crate::serde")
)]
pub struct WalletChangeSet {
    /// Descriptor for recipient addresses.
    pub descriptor: Option<miniscript::Descriptor<miniscript::DescriptorPublicKey>>,
    /// Descriptor for change addresses.
    pub change_descriptor: Option<miniscript::Descriptor<miniscript::DescriptorPublicKey>>,
    /// Stores the network type of the transaction data.
    pub network: Option<bitcoin::Network>,
    /// Changes to the [`LocalChain`](crate::local_chain::LocalChain).
    pub local_chain: crate::local_chain::ChangeSet,
    /// Changes to [`TxGraph`](crate::tx_graph::TxGraph).
    pub tx_graph: crate::tx_graph::ChangeSet<crate::ConfirmationBlockTime>,
    /// Changes to [`KeychainTxOutIndex`](crate::keychain_txout::KeychainTxOutIndex).
    pub indexer: crate::keychain_txout::ChangeSet,
}

impl Merge for WalletChangeSet {
    /// Merge another [`WalletChangeSet`] into itself.
    ///
    /// The `keychains_added` field respects the invariants of... TODO: FINISH THIS!
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
        }
        if other.network.is_some() {
            debug_assert!(
                self.network.is_none() || self.network == other.network,
                "network must never change"
            );
            self.network = other.network;
        }

        crate::Merge::merge(&mut self.local_chain, other.local_chain);
        crate::Merge::merge(&mut self.tx_graph, other.tx_graph);
        crate::Merge::merge(&mut self.indexer, other.indexer);
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

#[cfg(feature = "sqlite")]
impl WalletChangeSet {
    /// Schema name for wallet.
    pub const WALLET_SCHEMA_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet descriptors and network.
    pub const WALLET_TABLE_NAME: &'static str = "bdk_wallet";

    /// Initialize sqlite tables for wallet schema & table.
    fn init_wallet_sqlite_tables(db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        let schema_v0: &[&str] = &[&format!(
            "CREATE TABLE {} ( \
                id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
                descriptor TEXT, \
                change_descriptor TEXT, \
                network TEXT \
                ) STRICT;",
            Self::WALLET_TABLE_NAME,
        )];
        crate::sqlite::migrate_schema(db_tx, Self::WALLET_SCHEMA_NAME, &[schema_v0])
    }

    /// Recover a [`WalletChangeSet`] from sqlite database.
    pub fn from_sqlite(db_tx: &rusqlite::Transaction) -> rusqlite::Result<Self> {
        Self::init_wallet_sqlite_tables(db_tx)?;
        use crate::sqlite::Sql;
        use miniscript::{Descriptor, DescriptorPublicKey};
        use rusqlite::OptionalExtension;

        let mut changeset = Self::default();

        let mut wallet_statement = db_tx.prepare(&format!(
            "SELECT descriptor, change_descriptor, network FROM {}",
            Self::WALLET_TABLE_NAME,
        ))?;
        let row = wallet_statement
            .query_row([], |row| {
                Ok((
                    row.get::<_, Sql<Descriptor<DescriptorPublicKey>>>("descriptor")?,
                    row.get::<_, Sql<Descriptor<DescriptorPublicKey>>>("change_descriptor")?,
                    row.get::<_, Sql<bitcoin::Network>>("network")?,
                ))
            })
            .optional()?;
        if let Some((Sql(desc), Sql(change_desc), Sql(network))) = row {
            changeset.descriptor = Some(desc);
            changeset.change_descriptor = Some(change_desc);
            changeset.network = Some(network);
        }

        changeset.local_chain = crate::local_chain::ChangeSet::from_sqlite(db_tx)?;
        changeset.tx_graph = crate::tx_graph::ChangeSet::<_>::from_sqlite(db_tx)?;
        changeset.indexer = crate::indexer::keychain_txout::ChangeSet::from_sqlite(db_tx)?;

        Ok(changeset)
    }

    /// Persist [`WalletChangeSet`] to sqlite database.
    pub fn persist_to_sqlite(&self, db_tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
        Self::init_wallet_sqlite_tables(db_tx)?;
        use crate::sqlite::Sql;
        use rusqlite::named_params;

        let mut descriptor_statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, descriptor) VALUES(:id, :descriptor) ON CONFLICT(id) DO UPDATE SET descriptor=:descriptor",
            Self::WALLET_TABLE_NAME,
        ))?;
        if let Some(descriptor) = &self.descriptor {
            descriptor_statement.execute(named_params! {
                ":id": 0,
                ":descriptor": Sql(descriptor.clone()),
            })?;
        }

        let mut change_descriptor_statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, change_descriptor) VALUES(:id, :change_descriptor) ON CONFLICT(id) DO UPDATE SET change_descriptor=:change_descriptor",
            Self::WALLET_TABLE_NAME,
        ))?;
        if let Some(change_descriptor) = &self.change_descriptor {
            change_descriptor_statement.execute(named_params! {
                ":id": 0,
                ":change_descriptor": Sql(change_descriptor.clone()),
            })?;
        }

        let mut network_statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, network) VALUES(:id, :network) ON CONFLICT(id) DO UPDATE SET network=:network",
            Self::WALLET_TABLE_NAME,
        ))?;
        if let Some(network) = self.network {
            network_statement.execute(named_params! {
                ":id": 0,
                ":network": Sql(network),
            })?;
        }

        self.local_chain.persist_to_sqlite(db_tx)?;
        self.tx_graph.persist_to_sqlite(db_tx)?;
        self.indexer.persist_to_sqlite(db_tx)?;
        Ok(())
    }
}

impl From<crate::local_chain::ChangeSet> for WalletChangeSet {
    fn from(chain: crate::local_chain::ChangeSet) -> Self {
        Self {
            local_chain: chain,
            ..Default::default()
        }
    }
}

impl From<IndexedTxGraphChangeSet> for WalletChangeSet {
    fn from(indexed_tx_graph: IndexedTxGraphChangeSet) -> Self {
        Self {
            tx_graph: indexed_tx_graph.tx_graph,
            indexer: indexed_tx_graph.indexer,
            ..Default::default()
        }
    }
}

impl From<crate::tx_graph::ChangeSet<ConfirmationBlockTime>> for WalletChangeSet {
    fn from(tx_graph: crate::tx_graph::ChangeSet<ConfirmationBlockTime>) -> Self {
        Self {
            tx_graph,
            ..Default::default()
        }
    }
}

impl From<crate::keychain_txout::ChangeSet> for WalletChangeSet {
    fn from(indexer: crate::keychain_txout::ChangeSet) -> Self {
        Self {
            indexer,
            ..Default::default()
        }
    }
}
