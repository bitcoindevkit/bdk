use bdk_chain::{
    indexed_tx_graph, keychain_txout, local_chain, tx_graph, ConfirmationBlockTime, Merge,
};
use bitcoin::Txid;
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::wallet::tx_details;

type IndexedTxGraphChangeSet =
    indexed_tx_graph::ChangeSet<ConfirmationBlockTime, keychain_txout::ChangeSet>;

/// A changeset for [`Wallet`](crate::Wallet).
#[derive(Default, Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ChangeSet {
    /// Descriptor for recipient addresses.
    pub descriptor: Option<Descriptor<DescriptorPublicKey>>,
    /// Descriptor for change addresses.
    pub change_descriptor: Option<Descriptor<DescriptorPublicKey>>,
    /// Stores the network type of the transaction data.
    pub network: Option<bitcoin::Network>,
    /// Details and metadata of wallet transactions
    pub tx_details: Option<tx_details::ChangeSet>,

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

        match (&mut self.tx_details, other.tx_details) {
            (None, b) => self.tx_details = b,
            (Some(a), Some(b)) => Merge::merge(a, b),
            _ => {}
        }

        Merge::merge(&mut self.local_chain, other.local_chain);
        Merge::merge(&mut self.tx_graph, other.tx_graph);
        Merge::merge(&mut self.indexer, other.indexer);
    }

    fn is_empty(&self) -> bool {
        self.descriptor.is_none()
            && self.change_descriptor.is_none()
            && self.network.is_none()
            && (self.tx_details.is_none() || self.tx_details.as_ref().unwrap().is_empty())
            && self.local_chain.is_empty()
            && self.tx_graph.is_empty()
            && self.indexer.is_empty()
    }
}

#[cfg(feature = "rusqlite")]
impl ChangeSet {
    /// Schema name for wallet.
    pub const WALLET_SCHEMA_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet descriptors and network.
    pub const WALLET_TABLE_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet tx details
    pub const TX_DETAILS_TABLE_NAME: &'static str = "bdk_tx_details";

    /// Initialize sqlite tables for wallet tables.
    pub fn init_sqlite_tables(db_tx: &chain::rusqlite::Transaction) -> chain::rusqlite::Result<()> {
        let schema_v0: &[&str] = &[&format!(
            "CREATE TABLE {} ( \
                id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
                descriptor TEXT, \
                change_descriptor TEXT, \
                network TEXT \
                ) STRICT;",
            Self::WALLET_TABLE_NAME,
        )];
        // schema_v1 adds a table for tx-details
        let schema_v1: &[&str] = &[&format!(
            "CREATE TABLE {} ( \
                txid TEXT PRIMARY KEY NOT NULL, \
                is_canceled INTEGER DEFAULT 0 \
                ) STRICT;",
            Self::TX_DETAILS_TABLE_NAME,
        )];
        crate::rusqlite_impl::migrate_schema(
            db_tx,
            Self::WALLET_SCHEMA_NAME,
            &[schema_v0, schema_v1],
        )?;

        bdk_chain::local_chain::ChangeSet::init_sqlite_tables(db_tx)?;
        bdk_chain::tx_graph::ChangeSet::<ConfirmationBlockTime>::init_sqlite_tables(db_tx)?;
        bdk_chain::keychain_txout::ChangeSet::init_sqlite_tables(db_tx)?;

        Ok(())
    }

    /// Recover a [`ChangeSet`] from sqlite database.
    pub fn from_sqlite(db_tx: &chain::rusqlite::Transaction) -> chain::rusqlite::Result<Self> {
        use chain::rusqlite::OptionalExtension;
        use chain::Impl;

        let mut changeset = Self::default();

        let mut wallet_statement = db_tx.prepare(&format!(
            "SELECT descriptor, change_descriptor, network FROM {}",
            Self::WALLET_TABLE_NAME,
        ))?;
        let row = wallet_statement
            .query_row([], |row| {
                Ok((
                    row.get::<_, Option<Impl<Descriptor<DescriptorPublicKey>>>>("descriptor")?,
                    row.get::<_, Option<Impl<Descriptor<DescriptorPublicKey>>>>(
                        "change_descriptor",
                    )?,
                    row.get::<_, Option<Impl<bitcoin::Network>>>("network")?,
                ))
            })
            .optional()?;
        if let Some((desc, change_desc, network)) = row {
            changeset.descriptor = desc.map(Impl::into_inner);
            changeset.change_descriptor = change_desc.map(Impl::into_inner);
            changeset.network = network.map(Impl::into_inner);
        }

        // select tx details
        let mut change = tx_details::ChangeSet::default();
        let mut stmt = db_tx.prepare_cached(&format!(
            "SELECT txid, is_canceled FROM {}",
            Self::TX_DETAILS_TABLE_NAME,
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, Impl<Txid>>("txid")?,
                row.get::<_, bool>("is_canceled")?,
            ))
        })?;
        for res in rows {
            let (Impl(txid), is_canceled) = res?;
            let det = tx_details::Details { is_canceled };
            let record = tx_details::Record::Details(det);
            change.records.push((txid, record));
        }
        changeset.tx_details = if change.is_empty() {
            None
        } else {
            Some(change)
        };

        changeset.local_chain = local_chain::ChangeSet::from_sqlite(db_tx)?;
        changeset.tx_graph = tx_graph::ChangeSet::<_>::from_sqlite(db_tx)?;
        changeset.indexer = keychain_txout::ChangeSet::from_sqlite(db_tx)?;

        Ok(changeset)
    }

    /// Persist [`ChangeSet`] to sqlite database.
    pub fn persist_to_sqlite(
        &self,
        db_tx: &chain::rusqlite::Transaction,
    ) -> chain::rusqlite::Result<()> {
        use chain::rusqlite::named_params;
        use chain::Impl;

        let mut descriptor_statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, descriptor) VALUES(:id, :descriptor) ON CONFLICT(id) DO UPDATE SET descriptor=:descriptor",
            Self::WALLET_TABLE_NAME,
        ))?;
        if let Some(descriptor) = &self.descriptor {
            descriptor_statement.execute(named_params! {
                ":id": 0,
                ":descriptor": Impl(descriptor.clone()),
            })?;
        }

        let mut change_descriptor_statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, change_descriptor) VALUES(:id, :change_descriptor) ON CONFLICT(id) DO UPDATE SET change_descriptor=:change_descriptor",
            Self::WALLET_TABLE_NAME,
        ))?;
        if let Some(change_descriptor) = &self.change_descriptor {
            change_descriptor_statement.execute(named_params! {
                ":id": 0,
                ":change_descriptor": Impl(change_descriptor.clone()),
            })?;
        }

        let mut network_statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, network) VALUES(:id, :network) ON CONFLICT(id) DO UPDATE SET network=:network",
            Self::WALLET_TABLE_NAME,
        ))?;
        if let Some(network) = self.network {
            network_statement.execute(named_params! {
                ":id": 0,
                ":network": Impl(network),
            })?;
        }

        // persist tx details
        let mut cancel_tx_stmt = db_tx.prepare_cached(&format!(
            "REPLACE INTO {}(txid, is_canceled) VALUES(:txid, 1)",
            Self::TX_DETAILS_TABLE_NAME,
        ))?;
        if let Some(change) = &self.tx_details {
            for (txid, record) in &change.records {
                if record == &tx_details::Record::Canceled {
                    cancel_tx_stmt.execute(named_params! {
                        ":txid": Impl(*txid),
                    })?;
                }
            }
        }

        self.local_chain.persist_to_sqlite(db_tx)?;
        self.tx_graph.persist_to_sqlite(db_tx)?;
        self.indexer.persist_to_sqlite(db_tx)?;
        Ok(())
    }
}

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

impl From<tx_details::ChangeSet> for ChangeSet {
    fn from(value: tx_details::ChangeSet) -> Self {
        Self {
            tx_details: Some(value),
            ..Default::default()
        }
    }
}
