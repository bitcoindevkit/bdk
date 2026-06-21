//! This module provides utility functions for testing custom persistence backends.
use crate::{block_id, hash};
use alloc::sync::Arc;
use bdk_chain::{
    bitcoin::{self, OutPoint},
    local_chain, tx_graph, ConfirmationBlockTime, Merge,
};

#[cfg(feature = "miniscript")]
use bdk_chain::{indexer::keychain_txout, DescriptorExt, SpkIterator};
use core::{cmp::PartialEq, fmt::Debug};

use core::error::Error as Err;

use crate::utils::{create_test_tx, create_txout};

#[cfg(feature = "miniscript")]
use crate::utils::{parse_descriptor, spk_at_index};

const ADDRS: [&str; 2] = [
    "bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5",
    "bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl",
];

/// A helper to check if a custom persistence backend persists `ChangeSet`s correctly.
///
/// This first tries to create a `Store` using `init`, `load`s from it and checks if
/// the result is an empty `ChangeSet`.
/// It then tries to `persist` each `ChangeSet` in `changesets` one by one, doing a `load`
/// each time and checks that the aggregated `ChangeSet` matches the one loaded.
///
/// Finally it closes the `Store`, reopens it using `init`, `load`s from it and checks if the loaded
/// `ChangeSet` matches the final aggregated `ChangeSet`.
pub fn assert_persist_changesets<C, Store, Init, Load, Persist>(
    init: Init,
    load: Load,
    persist: Persist,
    changesets: &[C],
) -> Result<(), String>
where
    C: Debug + PartialEq + Default + Merge + Clone,
    Init: Fn() -> Result<Store, Box<dyn Err>>,
    Load: Fn(&mut Store) -> Result<C, Box<dyn Err>>,
    Persist: Fn(&mut Store, &C) -> Result<(), Box<dyn Err>>,
{
    let mut merged_changeset = C::default();
    {
        let mut store = init().map_err(|err| {
            format!(
                "Encountered an error from the persister while initializing the store.\nGot:\n{}",
                err
            )
        })?;

        let init_changeset = load(&mut store).map_err(|err| format!("Encountered an error from the persister while loading from the new store.\nGot:\n{}",err))?;

        if init_changeset != C::default() {
            Err("Loading from a new store should return an empty changeset.")?;
        }

        for (i, changeset) in changesets.iter().enumerate() {
            persist(&mut store, changeset).map_err(|err| format!("Persisting changeset no. {} failed. Got an error from the persister instead:\n{} ", i+1, err) )?;

            merged_changeset.merge(changeset.clone());

            let persisted_changeset = load(&mut store).map_err(|err| format!("Encountered an error from the persister while loading (after persisting changeset no. {}).\nGot:\n {}", i+1, err))?;

            if persisted_changeset != merged_changeset {
                Err(format!(
                    "Persisting changeset no. {} failed.\nExpected:\n\n{:?}\n\n\nLoaded:\n\n{:?};",
                    i + 1,
                    merged_changeset,
                    persisted_changeset
                ))?;
            }
        }
    }

    let mut store = init().map_err(|err| {
        format!(
            "Encountered an error while reopening the store.\nGot:\n{}",
            err
        )
    })?;

    let persisted_changeset = load(&mut store).map_err(|err| {
        format!(
            "Unable to load the persisted changeset after reopening the store.\nGot an error from the persister:\n{}",
            err
        )
    })?;

    if persisted_changeset != merged_changeset {
        Err(format!(
            "Did not get the expected changeset after reopening the store and loading.\nExpected:\n\n{:?}\n\n\nLoaded:\n\n{:?};",
            merged_changeset, persisted_changeset
        ))?;
    }

    Ok(())
}

/// Get two [`tx_graph::ChangeSet`]s.
pub fn tx_graph_changesets() -> [tx_graph::ChangeSet<ConfirmationBlockTime>; 2] {
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
