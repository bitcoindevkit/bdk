use bdk_chain::{
    indexed_tx_graph, keychain_txout, local_chain, tx_graph, ConfirmationBlockTime, Merge,
};
use miniscript::{Descriptor, DescriptorPublicKey};

type IndexedTxGraphChangeSet =
    indexed_tx_graph::ChangeSet<ConfirmationBlockTime, keychain_txout::ChangeSet>;

/// A changeset for [`Wallet`].
///
/// The changeset is used to persist and recover changes and additions made during the
/// life of the wallet. The kinds of changes that frequently make up this set are new
/// transactions, blocks in the local chain, and derivation indexes. The changeset is
/// also responsible for transmitting persistent metadata such as the public descriptor(s)
/// and configured network. All of these are necessary for the correct functioning of a
/// wallet.
///
/// For greater efficiency the wallet is able to stage changes before committing them.
/// Many operations result in staged changes which require persistence on the part of the
/// user. These include address revelation, applying an [`Update`], and introducing
/// transactions and related data to the wallet. To get the staged changes see
/// [`Wallet::staged`] and similar methods. Once the changes are committed to the persistence
/// layer the contents of the stage should be discarded.
///
/// You should persist early and often generally speaking, however in principle there is
/// no limitation on the number or type of changes that can be staged prior to persistence
/// or the order in which they're staged. This is because changesets are designed to be
/// [merged]. The change that is eventually persisted will include the combined effect of
/// each change individually.
///
/// The [`ChangeSet`] is backwards compatible in the sense that it is intended to interoperate
/// with earlier versions of the library. Specifically this means that the orginal fields will
/// remain unchanged and that the addition of new fields corresponds to new feature upgrades.
///
/// The API of [`Wallet`] is designed to give the user control of what data to persist, when
/// to persist it, and to determine when committed changes can be safely discarded. You should
/// consider how your database handles partial or repeat writes, whether the persistence
/// operation proceeds atomically, and whether the order of writes matters among the fields
/// of the [`ChangeSet`]. BDK has no strict requirements regarding these details but
/// provides a straightforward solution in the case of [SQLite]. If implementing your own
/// persistence layer, please see the docs for [`PersistedWallet`] and [`WalletPersister`]
/// as a reference.
///
/// [merged]: bdk_chain::Merge
/// [`PersistedWallet`]: crate::PersistedWallet
/// [SQLite]: bdk_chain::rusqlite_impl
/// [`Update`]: crate::Update
/// [`WalletPersister`]: crate::WalletPersister
/// [`Wallet::staged`]: crate::Wallet::staged
/// [`Wallet`]: crate::Wallet
#[derive(Default, Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
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

#[cfg(feature = "rusqlite")]
impl ChangeSet {
    /// Schema name for wallet.
    pub const WALLET_SCHEMA_NAME: &'static str = "bdk_wallet";
    /// Name of table to store wallet descriptors and network.
    pub const WALLET_TABLE_NAME: &'static str = "bdk_wallet";

    /// Get v0 sqlite [ChangeSet] schema
    pub fn schema_v0() -> alloc::string::String {
        format!(
            "CREATE TABLE {} ( \
                id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0), \
                descriptor TEXT, \
                change_descriptor TEXT, \
                network TEXT \
                ) STRICT;",
            Self::WALLET_TABLE_NAME,
        )
    }

    /// Initialize sqlite tables for wallet tables.
    pub fn init_sqlite_tables(db_tx: &chain::rusqlite::Transaction) -> chain::rusqlite::Result<()> {
        crate::rusqlite_impl::migrate_schema(
            db_tx,
            Self::WALLET_SCHEMA_NAME,
            &[&Self::schema_v0()],
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
