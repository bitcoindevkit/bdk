use bdk_chain::bitcoin::consensus::{deserialize, serialize};
use bdk_chain::bitcoin::hashes::Hash;
use bdk_chain::bitcoin::{Amount, Network, OutPoint, ScriptBuf, Transaction, TxOut};
use bdk_chain::bitcoin::{BlockHash, Txid};
use bdk_chain::miniscript::descriptor::{Descriptor, DescriptorPublicKey};
use rusqlite::{named_params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::Error;
use bdk_chain::{
    indexed_tx_graph, keychain, local_chain, tx_graph, Anchor, Append, DescriptorExt, DescriptorId,
};
use bdk_persist::CombinedChangeSet;

/// Persists data in to a relational schema based [SQLite] database file.
///
/// The changesets loaded or stored represent changes to keychain and blockchain data.
///
/// [SQLite]: https://www.sqlite.org/index.html
pub struct Store<K, A> {
    // A rusqlite connection to the SQLite database. Uses a Mutex for thread safety.
    conn: Mutex<Connection>,
    keychain_marker: PhantomData<K>,
    anchor_marker: PhantomData<A>,
}

impl<K, A> Debug for Store<K, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.conn, f)
    }
}

impl<K, A> Store<K, A>
where
    K: Ord + for<'de> Deserialize<'de> + Serialize + Send,
    A: Anchor + for<'de> Deserialize<'de> + Serialize + Send,
{
    /// Creates a new store from a [`Connection`].
    pub fn new(mut conn: Connection) -> Result<Self, rusqlite::Error> {
        Self::migrate(&mut conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
            keychain_marker: Default::default(),
            anchor_marker: Default::default(),
        })
    }

    pub(crate) fn db_transaction(&mut self) -> Result<rusqlite::Transaction, Error> {
        let connection = self.conn.get_mut().expect("unlocked connection mutex");
        connection.transaction().map_err(Error::Sqlite)
    }
}

impl<K, A, C> bdk_persist::PersistBackend<C> for Store<K, A>
where
    K: Ord + for<'de> Deserialize<'de> + Serialize + Send,
    A: Anchor + for<'de> Deserialize<'de> + Serialize + Send,
    C: Clone + From<CombinedChangeSet<K, A>> + Into<CombinedChangeSet<K, A>>,
{
    fn write_changes(&mut self, changeset: &C) -> anyhow::Result<()> {
        self.write(&changeset.clone().into())
            .map_err(|e| anyhow::anyhow!(e).context("unable to write changes to sqlite database"))
    }

    fn load_from_persistence(&mut self) -> anyhow::Result<Option<C>> {
        self.read()
            .map(|c| c.map(Into::into))
            .map_err(|e| anyhow::anyhow!(e).context("unable to read changes from sqlite database"))
    }
}

/// Network table related functions.
impl<K, A> Store<K, A> {
    /// Insert [`Network`] for which all other tables data is valid.
    ///
    /// Error if trying to insert different network value.
    fn insert_network(
        current_network: &Option<Network>,
        db_transaction: &rusqlite::Transaction,
        network_changeset: &Option<Network>,
    ) -> Result<(), Error> {
        if let Some(network) = network_changeset {
            match current_network {
                // if no network change do nothing
                Some(current_network) if current_network == network => Ok(()),
                // if new network not the same as current, error
                Some(current_network) => Err(Error::Network {
                    expected: *current_network,
                    given: *network,
                }),
                // insert network if none exists
                None => {
                    let insert_network_stmt = &mut db_transaction
                        .prepare_cached("INSERT INTO network (name) VALUES (:name)")
                        .expect("insert network statement");
                    let name = network.to_string();
                    insert_network_stmt
                        .execute(named_params! {":name": name })
                        .map_err(Error::Sqlite)?;
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }

    /// Select the valid [`Network`] for this database, or `None` if not set.
    fn select_network(db_transaction: &rusqlite::Transaction) -> Result<Option<Network>, Error> {
        let mut select_network_stmt = db_transaction
            .prepare_cached("SELECT name FROM network WHERE rowid = 1")
            .expect("select network statement");

        let network = select_network_stmt
            .query_row([], |row| {
                let network = row.get_unwrap::<usize, String>(0);
                let network = Network::from_str(network.as_str()).expect("valid network");
                Ok(network)
            })
            .map_err(Error::Sqlite);
        match network {
            Ok(network) => Ok(Some(network)),
            Err(Error::Sqlite(rusqlite::Error::QueryReturnedNoRows)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Block table related functions.
impl<K, A> Store<K, A> {
    /// Insert or delete local chain blocks.
    ///
    /// Error if trying to insert existing block hash.
    fn insert_or_delete_blocks(
        db_transaction: &rusqlite::Transaction,
        chain_changeset: &local_chain::ChangeSet,
    ) -> Result<(), Error> {
        for (height, hash) in chain_changeset.iter() {
            match hash {
                // add new hash at height
                Some(hash) => {
                    let insert_block_stmt = &mut db_transaction
                        .prepare_cached("INSERT INTO block (hash, height) VALUES (:hash, :height)")
                        .expect("insert block statement");
                    let hash = hash.to_string();
                    insert_block_stmt
                        .execute(named_params! {":hash": hash, ":height": height })
                        .map_err(Error::Sqlite)?;
                }
                // delete block at height
                None => {
                    let delete_block_stmt = &mut db_transaction
                        .prepare_cached("DELETE FROM block WHERE height IS :height")
                        .expect("delete block statement");
                    delete_block_stmt
                        .execute(named_params! {":height": height })
                        .map_err(Error::Sqlite)?;
                }
            }
        }

        Ok(())
    }

    /// Select all blocks.
    fn select_blocks(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeMap<u32, Option<BlockHash>>, Error> {
        let mut select_blocks_stmt = db_transaction
            .prepare_cached("SELECT height, hash FROM block")
            .expect("select blocks statement");

        let blocks = select_blocks_stmt
            .query_map([], |row| {
                let height = row.get_unwrap::<usize, u32>(0);
                let hash = row.get_unwrap::<usize, String>(1);
                let hash = Some(BlockHash::from_str(hash.as_str()).expect("block hash"));
                Ok((height, hash))
            })
            .map_err(Error::Sqlite)?;
        blocks
            .into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }
}

/// Keychain table related functions.
///
/// The keychain objects are stored as [`JSONB`] data.
/// [`JSONB`]: https://sqlite.org/json1.html#jsonb
impl<K, A> Store<K, A>
where
    K: Ord + for<'de> Deserialize<'de> + Serialize + Send,
    A: Anchor + Send,
{
    /// Insert keychain with descriptor and last active index.
    ///
    /// If keychain exists only update last active index.
    fn insert_keychains(
        db_transaction: &rusqlite::Transaction,
        tx_graph_changeset: &indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    ) -> Result<(), Error> {
        let keychain_changeset = &tx_graph_changeset.indexer;
        for (keychain, descriptor) in keychain_changeset.keychains_added.iter() {
            let insert_keychain_stmt = &mut db_transaction
                .prepare_cached("INSERT INTO keychain (keychain, descriptor, descriptor_id) VALUES (jsonb(:keychain), :descriptor, :descriptor_id)")
                .expect("insert keychain statement");
            let keychain_json = serde_json::to_string(keychain).expect("keychain json");
            let descriptor_id = descriptor.descriptor_id().to_byte_array();
            let descriptor = descriptor.to_string();
            insert_keychain_stmt.execute(named_params! {":keychain": keychain_json, ":descriptor": descriptor, ":descriptor_id": descriptor_id })
                .map_err(Error::Sqlite)?;
        }
        Ok(())
    }

    /// Update descriptor last revealed index.
    fn update_last_revealed(
        db_transaction: &rusqlite::Transaction,
        tx_graph_changeset: &indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    ) -> Result<(), Error> {
        let keychain_changeset = &tx_graph_changeset.indexer;
        for (descriptor_id, last_revealed) in keychain_changeset.last_revealed.iter() {
            let update_last_revealed_stmt = &mut db_transaction
                .prepare_cached(
                    "UPDATE keychain SET last_revealed = :last_revealed
                              WHERE descriptor_id = :descriptor_id",
                )
                .expect("update last revealed statement");
            let descriptor_id = descriptor_id.to_byte_array();
            update_last_revealed_stmt.execute(named_params! {":descriptor_id": descriptor_id, ":last_revealed": * last_revealed })
                .map_err(Error::Sqlite)?;
        }
        Ok(())
    }

    /// Select keychains added.
    fn select_keychains(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeMap<K, Descriptor<DescriptorPublicKey>>, Error> {
        let mut select_keychains_added_stmt = db_transaction
            .prepare_cached("SELECT json(keychain), descriptor FROM keychain")
            .expect("select keychains statement");

        let keychains = select_keychains_added_stmt
            .query_map([], |row| {
                let keychain = row.get_unwrap::<usize, String>(0);
                let keychain = serde_json::from_str::<K>(keychain.as_str()).expect("keychain");
                let descriptor = row.get_unwrap::<usize, String>(1);
                let descriptor = Descriptor::from_str(descriptor.as_str()).expect("descriptor");
                Ok((keychain, descriptor))
            })
            .map_err(Error::Sqlite)?;
        keychains
            .into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }

    /// Select descriptor last revealed indexes.
    fn select_last_revealed(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeMap<DescriptorId, u32>, Error> {
        let mut select_last_revealed_stmt = db_transaction
            .prepare_cached(
                "SELECT descriptor, last_revealed FROM keychain WHERE last_revealed IS NOT NULL",
            )
            .expect("select last revealed statement");

        let last_revealed = select_last_revealed_stmt
            .query_map([], |row| {
                let descriptor = row.get_unwrap::<usize, String>(0);
                let descriptor = Descriptor::from_str(descriptor.as_str()).expect("descriptor");
                let descriptor_id = descriptor.descriptor_id();
                let last_revealed = row.get_unwrap::<usize, u32>(1);
                Ok((descriptor_id, last_revealed))
            })
            .map_err(Error::Sqlite)?;
        last_revealed
            .into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }
}

/// Tx (transaction) and txout (transaction output) table related functions.
impl<K, A> Store<K, A> {
    /// Insert transactions.
    ///
    /// Error if trying to insert existing txid.
    fn insert_txs(
        db_transaction: &rusqlite::Transaction,
        tx_graph_changeset: &indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    ) -> Result<(), Error> {
        for tx in tx_graph_changeset.graph.txs.iter() {
            let insert_tx_stmt = &mut db_transaction
                .prepare_cached("INSERT INTO tx (txid, whole_tx) VALUES (:txid, :whole_tx) ON CONFLICT (txid) DO UPDATE SET whole_tx = :whole_tx WHERE txid = :txid")
                .expect("insert or update tx whole_tx statement");
            let txid = tx.txid().to_string();
            let whole_tx = serialize(&tx);
            insert_tx_stmt
                .execute(named_params! {":txid": txid, ":whole_tx": whole_tx })
                .map_err(Error::Sqlite)?;
        }
        Ok(())
    }

    /// Select all transactions.
    fn select_txs(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeSet<Arc<Transaction>>, Error> {
        let mut select_tx_stmt = db_transaction
            .prepare_cached("SELECT whole_tx FROM tx WHERE whole_tx IS NOT NULL")
            .expect("select tx statement");

        let txs = select_tx_stmt
            .query_map([], |row| {
                let whole_tx = row.get_unwrap::<usize, Vec<u8>>(0);
                let whole_tx: Transaction = deserialize(&whole_tx).expect("transaction");
                Ok(Arc::new(whole_tx))
            })
            .map_err(Error::Sqlite)?;

        txs.into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }

    /// Select all transactions with last_seen values.
    fn select_last_seen(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeMap<Txid, u64>, Error> {
        // load tx last_seen
        let mut select_last_seen_stmt = db_transaction
            .prepare_cached("SELECT txid, last_seen FROM tx WHERE last_seen IS NOT NULL")
            .expect("select tx last seen statement");

        let last_seen = select_last_seen_stmt
            .query_map([], |row| {
                let txid = row.get_unwrap::<usize, String>(0);
                let txid = Txid::from_str(&txid).expect("txid");
                let last_seen = row.get_unwrap::<usize, u64>(1);
                Ok((txid, last_seen))
            })
            .map_err(Error::Sqlite)?;
        last_seen
            .into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }

    /// Insert txouts.
    ///
    /// Error if trying to insert existing outpoint.
    fn insert_txouts(
        db_transaction: &rusqlite::Transaction,
        tx_graph_changeset: &indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    ) -> Result<(), Error> {
        for txout in tx_graph_changeset.graph.txouts.iter() {
            let insert_txout_stmt = &mut db_transaction
                .prepare_cached("INSERT INTO txout (txid, vout, value, script) VALUES (:txid, :vout, :value, :script)")
                .expect("insert txout statement");
            let txid = txout.0.txid.to_string();
            let vout = txout.0.vout;
            let value = txout.1.value.to_sat();
            let script = txout.1.script_pubkey.as_bytes();
            insert_txout_stmt.execute(named_params! {":txid": txid, ":vout": vout, ":value": value, ":script": script })
                .map_err(Error::Sqlite)?;
        }
        Ok(())
    }

    /// Select all transaction outputs.
    fn select_txouts(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeMap<OutPoint, TxOut>, Error> {
        // load tx outs
        let mut select_txout_stmt = db_transaction
            .prepare_cached("SELECT txid, vout, value, script FROM txout")
            .expect("select txout statement");

        let txouts = select_txout_stmt
            .query_map([], |row| {
                let txid = row.get_unwrap::<usize, String>(0);
                let txid = Txid::from_str(&txid).expect("txid");
                let vout = row.get_unwrap::<usize, u32>(1);
                let outpoint = OutPoint::new(txid, vout);
                let value = row.get_unwrap::<usize, u64>(2);
                let script_pubkey = row.get_unwrap::<usize, Vec<u8>>(3);
                let script_pubkey = ScriptBuf::from_bytes(script_pubkey);
                let txout = TxOut {
                    value: Amount::from_sat(value),
                    script_pubkey,
                };
                Ok((outpoint, txout))
            })
            .map_err(Error::Sqlite)?;
        txouts
            .into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }

    /// Update transaction last seen times.
    fn update_last_seen(
        db_transaction: &rusqlite::Transaction,
        tx_graph_changeset: &indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    ) -> Result<(), Error> {
        for tx_last_seen in tx_graph_changeset.graph.last_seen.iter() {
            let insert_or_update_tx_stmt = &mut db_transaction
                .prepare_cached("INSERT INTO tx (txid, last_seen) VALUES (:txid, :last_seen) ON CONFLICT (txid) DO UPDATE SET last_seen = :last_seen WHERE txid = :txid")
                .expect("insert or update tx last_seen statement");
            let txid = tx_last_seen.0.to_string();
            let last_seen = *tx_last_seen.1;
            insert_or_update_tx_stmt
                .execute(named_params! {":txid": txid, ":last_seen": last_seen })
                .map_err(Error::Sqlite)?;
        }
        Ok(())
    }
}

/// Anchor table related functions.
impl<K, A> Store<K, A>
where
    K: Ord + for<'de> Deserialize<'de> + Serialize + Send,
    A: Anchor + for<'de> Deserialize<'de> + Serialize + Send,
{
    /// Insert anchors.
    fn insert_anchors(
        db_transaction: &rusqlite::Transaction,
        tx_graph_changeset: &indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    ) -> Result<(), Error> {
        // serde_json::to_string
        for anchor in tx_graph_changeset.graph.anchors.iter() {
            let insert_anchor_stmt = &mut db_transaction
                .prepare_cached("INSERT INTO anchor_tx (block_hash, anchor, txid) VALUES (:block_hash, jsonb(:anchor), :txid)")
                .expect("insert anchor statement");
            let block_hash = anchor.0.anchor_block().hash.to_string();
            let anchor_json = serde_json::to_string(&anchor.0).expect("anchor json");
            let txid = anchor.1.to_string();
            insert_anchor_stmt.execute(named_params! {":block_hash": block_hash, ":anchor": anchor_json, ":txid": txid })
                .map_err(Error::Sqlite)?;
        }
        Ok(())
    }

    /// Select all anchors.
    fn select_anchors(
        db_transaction: &rusqlite::Transaction,
    ) -> Result<BTreeSet<(A, Txid)>, Error> {
        // serde_json::from_str
        let mut select_anchor_stmt = db_transaction
            .prepare_cached("SELECT block_hash, json(anchor), txid FROM anchor_tx")
            .expect("select anchor statement");
        let anchors = select_anchor_stmt
            .query_map([], |row| {
                let hash = row.get_unwrap::<usize, String>(0);
                let hash = BlockHash::from_str(hash.as_str()).expect("block hash");
                let anchor = row.get_unwrap::<usize, String>(1);
                let anchor: A = serde_json::from_str(anchor.as_str()).expect("anchor");
                // double check anchor blob block hash matches
                assert_eq!(hash, anchor.anchor_block().hash);
                let txid = row.get_unwrap::<usize, String>(2);
                let txid = Txid::from_str(&txid).expect("txid");
                Ok((anchor, txid))
            })
            .map_err(Error::Sqlite)?;
        anchors
            .into_iter()
            .map(|row| row.map_err(Error::Sqlite))
            .collect()
    }
}

/// Functions to read and write all [`ChangeSet`] data.
impl<K, A> Store<K, A>
where
    K: Ord + for<'de> Deserialize<'de> + Serialize + Send,
    A: Anchor + for<'de> Deserialize<'de> + Serialize + Send,
{
    fn write(&mut self, changeset: &CombinedChangeSet<K, A>) -> Result<(), Error> {
        // no need to write anything if changeset is empty
        if changeset.is_empty() {
            return Ok(());
        }

        let db_transaction = self.db_transaction()?;

        let network_changeset = &changeset.network;
        let current_network = Self::select_network(&db_transaction)?;
        Self::insert_network(&current_network, &db_transaction, network_changeset)?;

        let chain_changeset = &changeset.chain;
        Self::insert_or_delete_blocks(&db_transaction, chain_changeset)?;

        let tx_graph_changeset = &changeset.indexed_tx_graph;
        Self::insert_keychains(&db_transaction, tx_graph_changeset)?;
        Self::update_last_revealed(&db_transaction, tx_graph_changeset)?;
        Self::insert_txs(&db_transaction, tx_graph_changeset)?;
        Self::insert_txouts(&db_transaction, tx_graph_changeset)?;
        Self::insert_anchors(&db_transaction, tx_graph_changeset)?;
        Self::update_last_seen(&db_transaction, tx_graph_changeset)?;
        db_transaction.commit().map_err(Error::Sqlite)
    }

    fn read(&mut self) -> Result<Option<CombinedChangeSet<K, A>>, Error> {
        let db_transaction = self.db_transaction()?;

        let network = Self::select_network(&db_transaction)?;
        let chain = Self::select_blocks(&db_transaction)?;
        let keychains_added = Self::select_keychains(&db_transaction)?;
        let last_revealed = Self::select_last_revealed(&db_transaction)?;
        let txs = Self::select_txs(&db_transaction)?;
        let last_seen = Self::select_last_seen(&db_transaction)?;
        let txouts = Self::select_txouts(&db_transaction)?;
        let anchors = Self::select_anchors(&db_transaction)?;

        let graph: tx_graph::ChangeSet<A> = tx_graph::ChangeSet {
            txs,
            txouts,
            anchors,
            last_seen,
        };

        let indexer: keychain::ChangeSet<K> = keychain::ChangeSet {
            keychains_added,
            last_revealed,
        };

        let indexed_tx_graph: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>> =
            indexed_tx_graph::ChangeSet { graph, indexer };

        if network.is_none() && chain.is_empty() && indexed_tx_graph.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CombinedChangeSet {
                chain,
                indexed_tx_graph,
                network,
            }))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::store::Append;
    use bdk_chain::bitcoin::consensus::encode::deserialize;
    use bdk_chain::bitcoin::constants::genesis_block;
    use bdk_chain::bitcoin::hashes::hex::FromHex;
    use bdk_chain::bitcoin::transaction::Transaction;
    use bdk_chain::bitcoin::Network::Testnet;
    use bdk_chain::bitcoin::{secp256k1, BlockHash, OutPoint};
    use bdk_chain::miniscript::Descriptor;
    use bdk_chain::{
        indexed_tx_graph, keychain, tx_graph, BlockId, ConfirmationHeightAnchor,
        ConfirmationTimeHeightAnchor, DescriptorExt,
    };
    use bdk_persist::PersistBackend;
    use std::str::FromStr;
    use std::sync::Arc;

    #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Hash, Debug, Serialize, Deserialize)]
    enum Keychain {
        External { account: u32, name: String },
        Internal { account: u32, name: String },
    }

    #[test]
    fn insert_and_load_aggregate_changesets_with_confirmation_time_height_anchor(
    ) -> anyhow::Result<()> {
        let (test_changesets, agg_test_changesets) =
            create_test_changesets(&|height, time, hash| ConfirmationTimeHeightAnchor {
                confirmation_height: height,
                confirmation_time: time,
                anchor_block: (height, hash).into(),
            });

        let conn = Connection::open_in_memory().expect("in memory connection");
        let mut store = Store::<Keychain, ConfirmationTimeHeightAnchor>::new(conn)
            .expect("create new memory db store");

        test_changesets.iter().for_each(|changeset| {
            store.write_changes(changeset).expect("write changeset");
        });

        let agg_changeset = store.load_from_persistence().expect("aggregated changeset");

        assert_eq!(agg_changeset, Some(agg_test_changesets));
        Ok(())
    }

    #[test]
    fn insert_and_load_aggregate_changesets_with_confirmation_height_anchor() -> anyhow::Result<()>
    {
        let (test_changesets, agg_test_changesets) =
            create_test_changesets(&|height, _time, hash| ConfirmationHeightAnchor {
                confirmation_height: height,
                anchor_block: (height, hash).into(),
            });

        let conn = Connection::open_in_memory().expect("in memory connection");
        let mut store = Store::<Keychain, ConfirmationHeightAnchor>::new(conn)
            .expect("create new memory db store");

        test_changesets.iter().for_each(|changeset| {
            store.write_changes(changeset).expect("write changeset");
        });

        let agg_changeset = store.load_from_persistence().expect("aggregated changeset");

        assert_eq!(agg_changeset, Some(agg_test_changesets));
        Ok(())
    }

    #[test]
    fn insert_and_load_aggregate_changesets_with_blockid_anchor() -> anyhow::Result<()> {
        let (test_changesets, agg_test_changesets) =
            create_test_changesets(&|height, _time, hash| BlockId { height, hash });

        let conn = Connection::open_in_memory().expect("in memory connection");
        let mut store = Store::<Keychain, BlockId>::new(conn).expect("create new memory db store");

        test_changesets.iter().for_each(|changeset| {
            store.write_changes(changeset).expect("write changeset");
        });

        let agg_changeset = store.load_from_persistence().expect("aggregated changeset");

        assert_eq!(agg_changeset, Some(agg_test_changesets));
        Ok(())
    }

    fn create_test_changesets<A: Anchor + Copy>(
        anchor_fn: &dyn Fn(u32, u64, BlockHash) -> A,
    ) -> (
        Vec<CombinedChangeSet<Keychain, A>>,
        CombinedChangeSet<Keychain, A>,
    ) {
        let secp = &secp256k1::Secp256k1::signing_only();

        let network_changeset = Some(Testnet);

        let block_hash_0: BlockHash = genesis_block(Testnet).block_hash();
        let block_hash_1 =
            BlockHash::from_str("00000000b873e79784647a6c82962c70d228557d24a747ea4d1b8bbe878e1206")
                .unwrap();
        let block_hash_2 =
            BlockHash::from_str("000000006c02c8ea6e4ff69651f7fcde348fb9d557a06e6957b65552002a7820")
                .unwrap();

        let block_changeset = [
            (0, Some(block_hash_0)),
            (1, Some(block_hash_1)),
            (2, Some(block_hash_2)),
        ]
        .into();

        let ext_keychain = Keychain::External {
            account: 0,
            name: "ext test".to_string(),
        };
        let (ext_desc, _ext_keymap) = Descriptor::parse_descriptor(secp, "wpkh(tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy/0/*)").unwrap();
        let ext_desc_id = ext_desc.descriptor_id();
        let int_keychain = Keychain::Internal {
            account: 0,
            name: "int test".to_string(),
        };
        let (int_desc, _int_keymap) = Descriptor::parse_descriptor(secp, "wpkh(tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy/1/*)").unwrap();
        let int_desc_id = int_desc.descriptor_id();

        let tx0_hex = Vec::<u8>::from_hex("01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000").unwrap();
        let tx0: Arc<Transaction> = Arc::new(deserialize(tx0_hex.as_slice()).unwrap());
        let tx1_hex = Vec::<u8>::from_hex("010000000001010000000000000000000000000000000000000000000000000000000000000000ffffffff025151feffffff0200f2052a010000001600149243f727dd5343293eb83174324019ec16c2630f0000000000000000776a24aa21a9ede2f61c3f71d1defd3fa999dfa36953755c690689799962b48bebd836974e8cf94c4fecc7daa2490047304402205e423a8754336ca99dbe16509b877ef1bf98d008836c725005b3c787c41ebe46022047246e4467ad7cc7f1ad98662afcaf14c115e0095a227c7b05c5182591c23e7e01000120000000000000000000000000000000000000000000000000000000000000000000000000").unwrap();
        let tx1: Arc<Transaction> = Arc::new(deserialize(tx1_hex.as_slice()).unwrap());
        let tx2_hex = Vec::<u8>::from_hex("01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0e0432e7494d010e062f503253482fffffffff0100f2052a010000002321038a7f6ef1c8ca0c588aa53fa860128077c9e6c11e6830f4d7ee4e763a56b7718fac00000000").unwrap();
        let tx2: Arc<Transaction> = Arc::new(deserialize(tx2_hex.as_slice()).unwrap());

        let outpoint0_0 = OutPoint::new(tx0.txid(), 0);
        let txout0_0 = tx0.output.first().unwrap().clone();
        let outpoint1_0 = OutPoint::new(tx1.txid(), 0);
        let txout1_0 = tx1.output.first().unwrap().clone();

        let anchor1 = anchor_fn(1, 1296667328, block_hash_1);
        let anchor2 = anchor_fn(2, 1296688946, block_hash_2);

        let tx_graph_changeset = tx_graph::ChangeSet::<A> {
            txs: [tx0.clone(), tx1.clone()].into(),
            txouts: [(outpoint0_0, txout0_0), (outpoint1_0, txout1_0)].into(),
            anchors: [(anchor1, tx0.txid()), (anchor1, tx1.txid())].into(),
            last_seen: [
                (tx0.txid(), 1598918400),
                (tx1.txid(), 1598919121),
                (tx2.txid(), 1608919121),
            ]
            .into(),
        };

        let keychain_changeset = keychain::ChangeSet {
            keychains_added: [(ext_keychain, ext_desc), (int_keychain, int_desc)].into(),
            last_revealed: [(ext_desc_id, 124), (int_desc_id, 421)].into(),
        };

        let graph_changeset: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<Keychain>> =
            indexed_tx_graph::ChangeSet {
                graph: tx_graph_changeset,
                indexer: keychain_changeset,
            };

        // test changesets to write to db
        let mut changesets = Vec::new();

        changesets.push(CombinedChangeSet {
            chain: block_changeset,
            indexed_tx_graph: graph_changeset,
            network: network_changeset,
        });

        // create changeset that sets the whole tx2 and updates it's lastseen where before there was only the txid and last_seen
        let tx_graph_changeset2 = tx_graph::ChangeSet::<A> {
            txs: [tx2.clone()].into(),
            txouts: BTreeMap::default(),
            anchors: BTreeSet::default(),
            last_seen: [(tx2.txid(), 1708919121)].into(),
        };

        let graph_changeset2: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<Keychain>> =
            indexed_tx_graph::ChangeSet {
                graph: tx_graph_changeset2,
                indexer: keychain::ChangeSet::default(),
            };

        changesets.push(CombinedChangeSet {
            chain: local_chain::ChangeSet::default(),
            indexed_tx_graph: graph_changeset2,
            network: None,
        });

        // create changeset that adds a new anchor2 for tx0 and tx1
        let tx_graph_changeset3 = tx_graph::ChangeSet::<A> {
            txs: BTreeSet::default(),
            txouts: BTreeMap::default(),
            anchors: [(anchor2, tx0.txid()), (anchor2, tx1.txid())].into(),
            last_seen: BTreeMap::default(),
        };

        let graph_changeset3: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<Keychain>> =
            indexed_tx_graph::ChangeSet {
                graph: tx_graph_changeset3,
                indexer: keychain::ChangeSet::default(),
            };

        changesets.push(CombinedChangeSet {
            chain: local_chain::ChangeSet::default(),
            indexed_tx_graph: graph_changeset3,
            network: None,
        });

        // aggregated test changesets
        let agg_test_changesets =
            changesets
                .iter()
                .fold(CombinedChangeSet::<Keychain, A>::default(), |mut i, cs| {
                    i.append(cs.clone());
                    i
                });

        (changesets, agg_test_changesets)
    }
}
