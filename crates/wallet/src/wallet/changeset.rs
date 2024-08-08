use bdk_chain::{
    collections::BTreeMap, indexed_tx_graph, keychain_txout, local_chain, tx_graph,
    ConfirmationBlockTime, Merge,
};
use miniscript::{Descriptor, DescriptorPublicKey};
use serde::{de::DeserializeOwned, Serialize};

type IndexedTxGraphChangeSet =
    indexed_tx_graph::ChangeSet<ConfirmationBlockTime, keychain_txout::ChangeSet>;

/// A changeset for [`Wallet`](crate::Wallet).
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(bound(
    deserialize = "K: Ord + serde::Deserialize<'de>",
    serialize = "K: Ord + serde::Serialize"
))]
#[non_exhaustive]
pub struct ChangeSet<K> {
    /// Descriptor for addresses.
    pub descriptors: BTreeMap<K, Descriptor<DescriptorPublicKey>>,
    /// Stores the network type of the transaction data.
    pub network: Option<bitcoin::Network>,
    /// Changes to the [`LocalChain`](local_chain::LocalChain).
    pub local_chain: local_chain::ChangeSet,
    /// Changes to [`TxGraph`](tx_graph::TxGraph).
    pub tx_graph: tx_graph::ChangeSet<ConfirmationBlockTime>,
    /// Changes to [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex).
    pub indexer: keychain_txout::ChangeSet,
}

impl<K> Default for ChangeSet<K> {
    fn default() -> Self {
        Self {
            descriptors: Default::default(),
            network: Default::default(),
            local_chain: Default::default(),
            tx_graph: Default::default(),
            indexer: Default::default(),
        }
    }
}

impl<K: Ord> Merge for ChangeSet<K> {
    /// Merge another [`ChangeSet`] into itself.
    fn merge(&mut self, other: Self) {
        for (k, descriptor) in other.descriptors {
            use bdk_chain::collections::btree_map::Entry;
            match self.descriptors.entry(k) {
                Entry::Vacant(entry) => {
                    entry.insert(descriptor);
                }
                Entry::Occupied(entry) => {
                    debug_assert_eq!(
                        entry.get(),
                        &descriptor,
                        "changeset must not replace existing descriptor under keychain"
                    );
                }
            }
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
        self.descriptors.is_empty()
            && self.network.is_none()
            && self.local_chain.is_empty()
            && self.tx_graph.is_empty()
            && self.indexer.is_empty()
    }
}

#[cfg(feature = "rusqlite")]
impl<K: Serialize + DeserializeOwned + Ord> ChangeSet<K> {
    /// Schema name for wallet.
    pub const WALLET_SCHEMA_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet descriptors and network.
    pub const WALLET_TABLE_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet network.
    pub const NETWORK_TABLE_NAME: &'static str = "bdk_network";
    /// Name of table to store descriptors and an associated label (used as the table key).
    pub const DESCRIPTOR_TABLE_NAME: &'static str = "bdk_descriptor";

    /// Initialize sqlite tables for wallet schema & table.
    fn init_wallet_sqlite_tables(
        db_tx: &chain::rusqlite::Transaction,
    ) -> chain::rusqlite::Result<()> {
        let schema_v0: &[&str] = &[&format!(
            "CREATE TABLE {} ( \
                id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
                descriptor TEXT, \
                change_descriptor TEXT, \
                network TEXT \
                ) STRICT",
            Self::WALLET_TABLE_NAME,
        )];
        // TODO: We can't really migrate from V0 to V1 unless type 'K' provides some sort of
        // mapping from 'KeychainKind'.
        let schema_v1: &[&str] = &[
            &format!(
                "CREATE TABLE {} ( \
                    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
                    network TEXT NOT NULL \
                ) STRICT",
                Self::NETWORK_TABLE_NAME,
            ),
            &format!(
                "CREATE TABLE {} ( \
                    label TEXT PRIMARY KEY NOT NULL, \
                    descriptor TEXT NOT NULL
                ) STRICT",
                Self::DESCRIPTOR_TABLE_NAME,
            ),
        ];
        crate::rusqlite_impl::migrate_schema(
            db_tx,
            Self::WALLET_SCHEMA_NAME,
            &[schema_v0, schema_v1],
        )
    }

    /// Recover a [`ChangeSet`] from sqlite database.
    pub fn from_sqlite(db_tx: &chain::rusqlite::Transaction) -> chain::rusqlite::Result<Self> {
        Self::init_wallet_sqlite_tables(db_tx)?;
        use chain::rusqlite::OptionalExtension;
        use chain::rusqlite_impl::SerdeImpl;
        use chain::Impl;

        let mut changeset = Self::default();

        let mut statement = db_tx.prepare(&format!(
            "SELECT label, descriptor FROM {}",
            Self::DESCRIPTOR_TABLE_NAME
        ))?;
        let row_iter = statement.query_map([], |row| {
            Ok((
                row.get::<_, SerdeImpl<K>>("label")?,
                row.get::<_, Impl<Descriptor<DescriptorPublicKey>>>("descriptor")?,
            ))
        })?;
        for row in row_iter {
            let (SerdeImpl(label), Impl(descriptor)) = row?;
            changeset.descriptors.insert(label, descriptor);
        }

        let mut statement =
            db_tx.prepare(&format!("SELECT network FROM {}", Self::NETWORK_TABLE_NAME))?;
        let row = statement
            .query_row([], |row| row.get::<_, Impl<bitcoin::Network>>("network"))
            .optional()?;
        if let Some(Impl(network)) = row {
            changeset.network = Some(network);
        }

        // let mut wallet_statement = db_tx.prepare(&format!(
        //     "SELECT descriptor, change_descriptor, network FROM {}",
        //     Self::WALLET_TABLE_NAME,
        // ))?;
        // let row = wallet_statement
        //     .query_row([], |row| {
        //         Ok((
        //             row.get::<_, Impl<Descriptor<DescriptorPublicKey>>>("descriptor")?,
        //             row.get::<_, Impl<Descriptor<DescriptorPublicKey>>>("change_descriptor")?,
        //             row.get::<_, Impl<bitcoin::Network>>("network")?,
        //         ))
        //     })
        //     .optional()?;
        // if let Some((Impl(desc), Impl(change_desc), Impl(network))) = row {
        //     changeset.descriptor = Some(desc);
        //     changeset.change_descriptor = Some(change_desc);
        //     changeset.network = Some(network);
        // }

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
        Self::init_wallet_sqlite_tables(db_tx)?;
        use chain::rusqlite::named_params;
        use chain::rusqlite_impl::SerdeImpl;
        use chain::Impl;

        let mut statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(label, descriptor) VALUES(:label, descriptor) ON CONFLICT(label) DO UPDATE SET descriptor=:descriptor",
            Self::DESCRIPTOR_TABLE_NAME,
        ))?;
        for (label, descriptor) in &self.descriptors {
            statement.execute(named_params! {
                ":label": SerdeImpl(label),
                ":descriptor": Impl(descriptor.clone()),
            })?;
        }
        let mut statement = db_tx.prepare_cached(&format!(
            "INSERT INTO {}(id, network) VALUES(:id, :network) ON CONFLICT(id) DO UPDATE SET network=:network",
            Self::NETWORK_TABLE_NAME,
        ))?;
        if let Some(network) = self.network {
            statement.execute(named_params! {
                ":id": 0,
                ":network": Impl(network),
            })?;
        }

        // let mut descriptor_statement = db_tx.prepare_cached(&format!(
        //     "INSERT INTO {}(id, descriptor) VALUES(:id, :descriptor) ON CONFLICT(id) DO UPDATE SET descriptor=:descriptor",
        //     Self::WALLET_TABLE_NAME,
        // ))?;
        // if let Some(descriptor) = &self.descriptor {
        //     descriptor_statement.execute(named_params! {
        //         ":id": 0,
        //         ":descriptor": Impl(descriptor.clone()),
        //     })?;
        // }
        //
        // let mut change_descriptor_statement = db_tx.prepare_cached(&format!(
        //     "INSERT INTO {}(id, change_descriptor) VALUES(:id, :change_descriptor) ON CONFLICT(id) DO UPDATE SET change_descriptor=:change_descriptor",
        //     Self::WALLET_TABLE_NAME,
        // ))?;
        // if let Some(change_descriptor) = &self.change_descriptor {
        //     change_descriptor_statement.execute(named_params! {
        //         ":id": 0,
        //         ":change_descriptor": Impl(change_descriptor.clone()),
        //     })?;
        // }
        //
        // let mut network_statement = db_tx.prepare_cached(&format!(
        //     "INSERT INTO {}(id, network) VALUES(:id, :network) ON CONFLICT(id) DO UPDATE SET network=:network",
        //     Self::WALLET_TABLE_NAME,
        // ))?;
        // if let Some(network) = self.network {
        //     network_statement.execute(named_params! {
        //         ":id": 0,
        //         ":network": Impl(network),
        //     })?;
        // }

        self.local_chain.persist_to_sqlite(db_tx)?;
        self.tx_graph.persist_to_sqlite(db_tx)?;
        self.indexer.persist_to_sqlite(db_tx)?;
        Ok(())
    }
}

impl<K> From<local_chain::ChangeSet> for ChangeSet<K> {
    fn from(chain: local_chain::ChangeSet) -> Self {
        Self {
            local_chain: chain,
            ..Default::default()
        }
    }
}

impl<K> From<IndexedTxGraphChangeSet> for ChangeSet<K> {
    fn from(indexed_tx_graph: IndexedTxGraphChangeSet) -> Self {
        Self {
            tx_graph: indexed_tx_graph.tx_graph,
            indexer: indexed_tx_graph.indexer,
            ..Default::default()
        }
    }
}

impl<K> From<tx_graph::ChangeSet<ConfirmationBlockTime>> for ChangeSet<K> {
    fn from(tx_graph: tx_graph::ChangeSet<ConfirmationBlockTime>) -> Self {
        Self {
            tx_graph,
            ..Default::default()
        }
    }
}

impl<K> From<keychain_txout::ChangeSet> for ChangeSet<K> {
    fn from(indexer: keychain_txout::ChangeSet) -> Self {
        Self {
            indexer,
            ..Default::default()
        }
    }
}
