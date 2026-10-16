//! This module provides utility functions for testing custom persistence backends.
use crate::{block_id, hash};
use alloc::sync::Arc;
use bdk_chain::{
    bitcoin::{self, transaction::Version, OutPoint},
    local_chain, tx_graph, ConfirmationBlockTime, Merge,
};

#[cfg(feature = "miniscript")]
use bdk_chain::{indexer::keychain_txout, DescriptorExt, SpkIterator};
use core::{cmp::PartialEq, error::Error, fmt::Debug};

use crate::utils::{create_test_tx, create_txout};

#[cfg(feature = "miniscript")]
use crate::utils::{parse_descriptor, spk_at_index};

const ADDRS: [&str; 2] = [
    "bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5",
    "bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl",
];

/// Asserts a persistence backend correctly stores and reloads a sequence of `ChangeSet`s.
///
/// Persists every changeset, drops and reopens the store, then checks that `load` returns the merge
/// of everything `persist`ed. The backend is supplied as three closures:
///
/// - `init`: open-or-create a handle. This must never wipe existing data.
/// - `load`: return the full persisted changeset (`C::default()` on a fresh store).
/// - `persist`: durably write one changeset on top of what's stored; must survive the handle being
///   dropped.
///
/// # Panics
///
/// Panics naming the failing step if a closure errors or a load doesn't match the expected merge.
#[track_caller]
pub fn assert_persist_changesets<C, Store, Init, Load, Persist>(
    init: Init,
    load: Load,
    persist: Persist,
    changesets: &[C],
) where
    C: Debug + PartialEq + Default + Merge + Clone,
    Init: Fn() -> Result<Store, Box<dyn Error>>,
    Load: Fn(&mut Store) -> Result<C, Box<dyn Error>>,
    Persist: Fn(&mut Store, &C) -> Result<(), Box<dyn Error>>,
{
    let mut store = match init() {
        Ok(store) => store,
        Err(err) => panic!("Failed to initialize the store: {err}"),
    };
    let init_changeset = match load(&mut store) {
        Ok(changeset) => changeset,
        Err(err) => panic!("Failed to load from the new store: {err}"),
    };
    assert!(
        init_changeset.is_empty(),
        "Loading from a new store should return an empty changeset."
    );

    let mut merged_changeset = C::default();
    for (i, changeset) in changesets.iter().enumerate() {
        if let Err(err) = persist(&mut store, changeset) {
            panic!("Failed to persist changeset no. {}: {err}", i + 1);
        }
        merged_changeset.merge(changeset.clone());
    }

    drop(store);
    let mut store = match init() {
        Ok(store) => store,
        Err(err) => panic!("Failed to reopen the store: {err}"),
    };
    let persisted_changeset = match load(&mut store) {
        Ok(changeset) => changeset,
        Err(err) => panic!("Failed to load after reopening the store: {err}"),
    };
    assert_eq!(
        persisted_changeset, merged_changeset,
        "Loaded changeset after reopening doesn't match what was persisted."
    );
}

/// Get two [`tx_graph::ChangeSet`]s.
pub fn tx_graph_changesets() -> [tx_graph::ChangeSet<ConfirmationBlockTime>; 2] {
    use tx_graph::ChangeSet;

    let tx1 = Arc::new(create_test_tx(
        [hash!("BTC")],
        [0],
        [30_000],
        [ADDRS[0]],
        Version::ONE,
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
        Version::ONE,
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

/// Get two [`keychain_txout::ChangeSet`]s.
#[cfg(feature = "miniscript")]
pub fn keychain_txout_changesets() -> [keychain_txout::ChangeSet; 2] {
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

/// Get two [`local_chain::ChangeSet`]s.
pub fn local_chain_changesets() -> [local_chain::ChangeSet; 2] {
    use local_chain::ChangeSet;

    let changeset = ChangeSet {
        blocks: [(910425, Some(hash!("B"))), (910426, Some(hash!("D")))].into(),
    };

    let changeset_new = ChangeSet {
        blocks: [(910427, Some(hash!("K")))].into(),
    };

    [changeset, changeset_new]
}
