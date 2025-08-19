//! This module provides utility functions for testing custom persistence backends.
use crate::{block_id, hash};
use alloc::sync::Arc;
use bdk_chain::{
    bitcoin::{self, OutPoint},
    local_chain, tx_graph, ConfirmationBlockTime, Merge,
};

#[cfg(feature = "miniscript")]
use bdk_chain::{indexer::keychain_txout, DescriptorExt, SpkIterator};
use core::{
    cmp::PartialEq,
    fmt::{Debug, Display},
};

use core::error::Error as Err;

use crate::utils::{create_test_tx, create_txout};

#[cfg(feature = "miniscript")]
use crate::utils::{parse_descriptor, spk_at_index};

const ADDRS: [&str; 2] = [
    "bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5",
    "bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl",
];

/// Errors caused by a failed persister test.
#[derive(Debug)]
pub enum PersistErr<C: Debug> {
    /// ChangeSet Mismatch
    ChangeSetMismatch {
        /// the resulting changeset
        got: Box<C>,
        /// the expected changeset
        expected: Box<C>,
    },
    /// Errors thrown by underlying persistence backend.
    Persister(Box<dyn Err + 'static + Send + Sync>),
}

impl<C: Debug> From<Box<dyn Err + 'static + Send + Sync>> for PersistErr<C> {
    fn from(value: Box<dyn Err + 'static + Send + Sync>) -> Self {
        PersistErr::Persister(value)
    }
}

impl<C: Debug> Display for PersistErr<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PersistErr::ChangeSetMismatch { got, expected } => write!(
                f,
                "ChangeSet mismatch! Got: {:?}, Expected: {:?}",
                got, expected
            ),
            PersistErr::Persister(err) => write!(f, "{err}"),
        }
    }
}

impl<C: Debug> Err for PersistErr<C> {}

/// Tests if `ChangeSet` is being persisted correctly.
///
/// We create a dummy `ChangeSet`, persist it and check if loaded `ChangeSet` matches
/// the persisted one. We then create another such dummy `ChangeSet`, persist it and load it to
/// check if merged `ChangeSet` is returned.
pub fn persist_changeset<CS, Store, CreateStore, Initialize, Persist>(
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
    changesets: &[CS],
) -> Result<(), PersistErr<CS>>
where
    CS: Debug + PartialEq + Default + Merge + Clone,
    CreateStore: Fn() -> Result<Store, Box<dyn Err + 'static + Send + Sync>>,
    Initialize: Fn(&mut Store) -> Result<CS, Box<dyn Err + 'static + Send + Sync>>,
    Persist: Fn(&mut Store, &CS) -> Result<(), Box<dyn Err + 'static + Send + Sync>>,
{
    let mut store = create_store()?;

    let init_changeset = initialize(&mut store)?;

    if init_changeset != CS::default() {
        return Err(PersistErr::ChangeSetMismatch {
            expected: Box::new(CS::default()),
            got: Box::new(init_changeset),
        });
    }

    let mut merged_changeset = CS::default();

    for changeset in changesets {
        persist(&mut store, changeset)?;
        merged_changeset.merge(changeset.clone());
    }

    let persisted_changeset = initialize(&mut store)?;

    if persisted_changeset != merged_changeset {
        return Err(PersistErr::ChangeSetMismatch {
            expected: Box::new(merged_changeset),
            got: Box::new(persisted_changeset),
        });
    }

    Ok(())
}

/// Tests if [`TxGraph`](tx_graph::TxGraph) is being persisted correctly.
///
/// We create a dummy [`tx_graph::ChangeSet`], persist it and check if loaded
/// `ChangeSet` matches the persisted one. We then create another such dummy `ChangeSet`, persist it
/// and load it to check if merged `ChangeSet` is returned.
pub fn persist_txgraph_changeset<Store, CreateStore, Initialize, Persist>(
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) -> Result<(), PersistErr<tx_graph::ChangeSet<ConfirmationBlockTime>>>
where
    CreateStore: Fn() -> Result<Store, Box<dyn Err + 'static + Send + Sync>>,
    Initialize: Fn(
        &mut Store,
    ) -> Result<
        tx_graph::ChangeSet<ConfirmationBlockTime>,
        Box<dyn Err + 'static + Send + Sync>,
    >,
    Persist: Fn(
        &mut Store,
        &tx_graph::ChangeSet<ConfirmationBlockTime>,
    ) -> Result<(), Box<dyn Err + 'static + Send + Sync>>,
{
    let changesets = tx_graph_changesets();
    persist_changeset::<
        tx_graph::ChangeSet<ConfirmationBlockTime>,
        Store,
        CreateStore,
        Initialize,
        Persist,
    >(create_store, initialize, persist, &changesets)
}

/// Tests if [`KeychainTxOutIndex`](keychain_txout::KeychainTxOutIndex) is being persisted
/// correctly.
///
/// See [`persist_txgraph_changeset`].
#[cfg(feature = "miniscript")]
pub fn persist_indexer_changeset<Store, CreateStore, Initialize, Persist>(
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) -> Result<(), PersistErr<keychain_txout::ChangeSet>>
where
    CreateStore: Fn() -> Result<Store, Box<dyn Err + 'static + Send + Sync>>,
    Initialize:
        Fn(&mut Store) -> Result<keychain_txout::ChangeSet, Box<dyn Err + 'static + Send + Sync>>,
    Persist: Fn(
        &mut Store,
        &keychain_txout::ChangeSet,
    ) -> Result<(), Box<dyn Err + 'static + Send + Sync>>,
{
    let changesets = keychain_txout_changesets();
    persist_changeset::<keychain_txout::ChangeSet, Store, CreateStore, Initialize, Persist>(
        create_store,
        initialize,
        persist,
        &changesets,
    )
}

/// Tests if [`LocalChain`](local_chain::LocalChain) is being persisted correctly.
///
/// See [`persist_txgraph_changeset`].
pub fn persist_local_chain_changeset<Store, CreateStore, Initialize, Persist>(
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) -> Result<(), PersistErr<local_chain::ChangeSet>>
where
    CreateStore: Fn() -> Result<Store, Box<dyn Err + 'static + Send + Sync>>,
    Initialize:
        Fn(&mut Store) -> Result<local_chain::ChangeSet, Box<dyn Err + 'static + Send + Sync>>,
    Persist:
        Fn(&mut Store, &local_chain::ChangeSet) -> Result<(), Box<dyn Err + 'static + Send + Sync>>,
{
    let changesets = local_chain_changesets();
    persist_changeset::<local_chain::ChangeSet, Store, CreateStore, Initialize, Persist>(
        create_store,
        initialize,
        persist,
        &changesets,
    )
}

/// Get two [`tx_graph::ChangeSet`](tx_graph::ChangeSet)s.
fn tx_graph_changesets() -> [tx_graph::ChangeSet<ConfirmationBlockTime>; 2] {
    use tx_graph::ChangeSet;

    let tx1 = Arc::new(create_test_tx(
        [hash!("BTC")],
        [0],
        [30_000],
        [ADDRS[0]],
        1,
        0,
    ));

    let conf_anchor: ConfirmationBlockTime = ConfirmationBlockTime {
        block_id: block_id!(910425, "Rust"),
        confirmation_time: 1755416660,
    };

    let tx_graph_changeset1 = ChangeSet::<ConfirmationBlockTime> {
        txs: [tx1.clone()].into(),
        txouts: [
            (OutPoint::new(hash!("BDK"), 0), create_txout(1300, ADDRS[1])),
            (
                OutPoint::new(hash!("Bitcoin_fixes_things"), 0),
                create_txout(1400, ADDRS[1]),
            ),
        ]
        .into(),
        anchors: [(conf_anchor, tx1.compute_txid())].into(),
        last_seen: [(tx1.compute_txid(), 1755416650)].into(),
        first_seen: [(tx1.compute_txid(), 1755416655)].into(),
        last_evicted: [(tx1.compute_txid(), 1755416660)].into(),
    };

    let tx2 = Arc::new(create_test_tx(
        [tx1.compute_txid()],
        [0],
        [20_000],
        [ADDRS[0]],
        1,
        0,
    ));

    let conf_anchor: ConfirmationBlockTime = ConfirmationBlockTime {
        block_id: block_id!(910426, "BOSS"),
        confirmation_time: 1755416700,
    };

    let tx_graph_changeset2 = ChangeSet::<ConfirmationBlockTime> {
        txs: [tx2.clone()].into(),
        txouts: [(
            OutPoint::new(hash!("Magical_Bitcoin"), 0),
            create_txout(10000, ADDRS[1]),
        )]
        .into(),
        anchors: [(conf_anchor, tx2.compute_txid())].into(),
        last_seen: [(tx2.compute_txid(), 1755416700)].into(),
        first_seen: [(tx2.compute_txid(), 1755416670)].into(),
        last_evicted: [(tx2.compute_txid(), 1755416760)].into(),
    };

    [tx_graph_changeset1, tx_graph_changeset2]
}

/// Get two [`keychain_txout::ChangeSet`](keychain_txout::ChangeSet)s.
#[cfg(feature = "miniscript")]
fn keychain_txout_changesets() -> [keychain_txout::ChangeSet; 2] {
    use crate::utils::DESCRIPTORS;
    use keychain_txout::ChangeSet;

    let descriptor_ids = DESCRIPTORS.map(|d| parse_descriptor(d).0.descriptor_id());
    let descs = DESCRIPTORS.map(|desc| parse_descriptor(desc).0);

    let changeset = ChangeSet {
        last_revealed: [(descriptor_ids[0], 1), (descriptor_ids[1], 100)].into(),
        spk_cache: [
            (
                descriptor_ids[0],
                SpkIterator::new_with_range(&descs[0], 0..=26).collect(),
            ),
            (
                descriptor_ids[1],
                SpkIterator::new_with_range(&descs[1], 0..=125).collect(),
            ),
        ]
        .into(),
    };

    let changeset_new = ChangeSet {
        last_revealed: [(descriptor_ids[0], 2)].into(),
        spk_cache: [(
            descriptor_ids[0],
            [(27, spk_at_index(&descs[0], 27))].into(),
        )]
        .into(),
    };

    [changeset, changeset_new]
}

/// Get two [`local_chain::ChangeSet`](local_chain::ChangeSet)s.
fn local_chain_changesets() -> [local_chain::ChangeSet; 2] {
    use local_chain::ChangeSet;

    let changeset = ChangeSet {
        blocks: [(910425, Some(hash!("B"))), (910426, Some(hash!("D")))].into(),
    };

    let changeset_new = ChangeSet {
        blocks: [(910427, Some(hash!("K")))].into(),
    };

    [changeset, changeset_new]
}
