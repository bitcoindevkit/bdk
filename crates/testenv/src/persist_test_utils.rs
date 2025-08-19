use crate::block_id;
use crate::hash;
use bdk_chain::bitcoin;
use bdk_chain::miniscript::{Descriptor, DescriptorPublicKey};
use bdk_chain::{
    bitcoin::{
        absolute, key::Secp256k1, transaction, Address, Amount, OutPoint, ScriptBuf, Transaction,
        TxIn, TxOut, Txid,
    },
    indexer::keychain_txout,
    local_chain, tx_graph, ConfirmationBlockTime, DescriptorExt, DescriptorId, Merge, SpkIterator,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

fn create_one_inp_one_out_tx(txid: Txid, amount: u64) -> Transaction {
    Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(txid, 0),
            ..TxIn::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: Address::from_str("bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5")
                .unwrap()
                .assume_checked()
                .script_pubkey(),
        }],
    }
}

fn spk_at_index(descriptor: &Descriptor<DescriptorPublicKey>, index: u32) -> ScriptBuf {
    descriptor
        .derived_descriptor(&Secp256k1::verification_only(), index)
        .expect("must derive")
        .script_pubkey()
}

pub fn persist_txgraph_changeset<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should load empty changeset");
    assert_eq!(changeset, ChangeSet::<ConfirmationBlockTime>::default());

    let tx1 = Arc::new(create_one_inp_one_out_tx(hash!("BTC"), 30_000));

    let conf_anchor: ConfirmationBlockTime = ConfirmationBlockTime {
        block_id: block_id!(910425, "Rust"),
        confirmation_time: 1755416660,
    };

    let mut tx_graph_changeset1 = ChangeSet::<ConfirmationBlockTime> {
        txs: [tx1.clone()].into(),
        txouts: [
            (
                OutPoint::new(hash!("BDK"), 0),
                TxOut {
                    value: Amount::from_sat(1300),
                    script_pubkey: Address::from_str(
                        "bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl",
                    )
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
                },
            ),
            (
                OutPoint::new(hash!("Bitcoin_fixes_things"), 0),
                TxOut {
                    value: Amount::from_sat(1400),
                    script_pubkey: Address::from_str(
                        "bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl",
                    )
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
                },
            ),
        ]
        .into(),
        anchors: [(conf_anchor, tx1.compute_txid())].into(),
        last_seen: [(tx1.compute_txid(), 1755416650)].into(),
        first_seen: [(tx1.compute_txid(), 1755416655)].into(),
        last_evicted: [(tx1.compute_txid(), 1755416660)].into(),
    };

    persist(&mut store, &tx_graph_changeset1).expect("should persist changeset");

    let changeset = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset, tx_graph_changeset1);

    let tx2 = Arc::new(create_one_inp_one_out_tx(tx1.compute_txid(), 20_000));

    let conf_anchor: ConfirmationBlockTime = ConfirmationBlockTime {
        block_id: block_id!(910426, "BOSS"),
        confirmation_time: 1755416700,
    };

    let tx_graph_changeset2 = ChangeSet::<ConfirmationBlockTime> {
        txs: [tx2.clone()].into(),
        txouts: [(
            OutPoint::new(hash!("REDB"), 0),
            TxOut {
                value: Amount::from_sat(10000),
                script_pubkey: Address::from_str("bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl")
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
            },
        )]
        .into(),
        anchors: [(conf_anchor, tx2.compute_txid())].into(),
        last_seen: [(tx2.compute_txid(), 1755416700)].into(),
        first_seen: [(tx2.compute_txid(), 1755416670)].into(),
        last_evicted: [(tx2.compute_txid(), 1755416760)].into(),
    };

    persist(&mut store, &tx_graph_changeset2).expect("should persist changeset");

    let changeset = initialize(&mut store).expect("should load persisted changeset");

    tx_graph_changeset1.merge(tx_graph_changeset2);

    assert_eq!(tx_graph_changeset1, changeset);
}

fn parse_descriptor(descriptor: &str) -> Descriptor<DescriptorPublicKey> {
    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, descriptor)
        .unwrap()
        .0
}

pub fn persist_indexer_changeset<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<keychain_txout::ChangeSet>,
    Persist: Fn(&mut Store, &keychain_txout::ChangeSet) -> anyhow::Result<()>,
{
    use crate::utils::DESCRIPTORS;
    use keychain_txout::ChangeSet;

    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should load empty changeset");
    assert_eq!(changeset, ChangeSet::default());

    let descriptor_ids = DESCRIPTORS.map(|d| parse_descriptor(d).descriptor_id());
    let descs = DESCRIPTORS.map(parse_descriptor);

    let mut changeset = ChangeSet {
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

    persist(&mut store, &changeset).expect("should persist keychain_txout");

    let changeset_read = initialize(&mut store).expect("should load persisted changeset");

    assert_eq!(changeset_read, changeset);

    let changeset_new = ChangeSet {
        last_revealed: [(descriptor_ids[0], 2)].into(),
        spk_cache: [(
            descriptor_ids[0],
            [(27, spk_at_index(&descs[0], 27))].into(),
        )]
        .into(),
    };

    persist(&mut store, &changeset_new).expect("should persist second changeset");

    let changeset_read_new = initialize(&mut store).expect("should load merged changesets");
    changeset.merge(changeset_new);

    assert_eq!(changeset_read_new, changeset);
}

pub fn persist_local_chain_changeset<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<local_chain::ChangeSet>,
    Persist: Fn(&mut Store, &local_chain::ChangeSet) -> anyhow::Result<()>,
{
    use local_chain::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should load empty changeset");
    assert_eq!(changeset, ChangeSet::default());

    let changeset = ChangeSet {
        blocks: [(910425, Some(hash!("B"))), (910426, Some(hash!("D")))].into(),
    };

    persist(&mut store, &changeset).expect("should persist changeset");

    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read, changeset);

    // create another local_chain_changeset, persist that and read it
    let changeset_new = ChangeSet {
        blocks: [(910427, Some(hash!("K")))].into(),
    };

    persist(&mut store, &changeset_new).expect("should persist changeset");

    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");

    let changeset = ChangeSet {
        blocks: [
            (910425, Some(hash!("B"))),
            (910426, Some(hash!("D"))),
            (910427, Some(hash!("K"))),
        ]
        .into(),
    };

    assert_eq!(changeset, changeset_read_new);
}

pub fn persist_last_seen<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset =
        initialize(&mut store).expect("store should initialize and we should get empty changeset");
    assert_eq!(changeset, ChangeSet::<ConfirmationBlockTime>::default());

    let tx1 = Arc::new(create_one_inp_one_out_tx(hash!("BTC"), 30_000));
    let tx2 = Arc::new(create_one_inp_one_out_tx(tx1.compute_txid(), 20_000));
    let tx3 = Arc::new(create_one_inp_one_out_tx(tx2.compute_txid(), 19_000));

    // try persisting and reading last_seen
    let txs: BTreeSet<Arc<Transaction>> = [tx1.clone(), tx2.clone()].into();
    let mut last_seen: BTreeMap<Txid, u64> = [
        (tx1.compute_txid(), 1755416700),
        (tx2.compute_txid(), 1755416800),
    ]
    .into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs,
        last_seen: last_seen.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };
    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.last_seen, last_seen);

    // persist another last_seen and see if what is read is same as merged one
    let txs_new: BTreeSet<Arc<Transaction>> = [tx3.clone()].into();
    let last_seen_new: BTreeMap<Txid, u64> = [(tx3.compute_txid(), 1755417800)].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs: txs_new,
        last_seen: last_seen_new.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };
    persist(&mut store, &changeset).expect("should persist changeset");

    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");
    last_seen.merge(last_seen_new);
    assert_eq!(changeset_read_new.last_seen, last_seen);
}

pub fn persist_last_evicted<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset =
        initialize(&mut store).expect("store should initialize and we should get empty changeset");
    assert_eq!(changeset, ChangeSet::<ConfirmationBlockTime>::default());

    let tx1 = Arc::new(create_one_inp_one_out_tx(hash!("BDK"), 30_000));
    let tx2 = Arc::new(create_one_inp_one_out_tx(tx1.compute_txid(), 20_000));
    let tx3 = Arc::new(create_one_inp_one_out_tx(tx2.compute_txid(), 19_000));

    // try persisting and reading last_evicted
    let mut last_evicted: BTreeMap<Txid, u64> = [
        (tx1.compute_txid(), 1755416600),
        (tx2.compute_txid(), 1755416060),
    ]
    .into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        last_evicted: last_evicted.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };
    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.last_evicted, last_evicted);

    // persist another last_evicted and see if what is read is same as merged one
    let last_evicted_new: BTreeMap<Txid, u64> = [(tx3.compute_txid(), 1755416700)].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        last_evicted: last_evicted_new.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };
    persist(&mut store, &changeset).expect("should persist changeset");

    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");
    last_evicted.merge(last_evicted_new);
    assert_eq!(changeset_read_new.last_evicted, last_evicted);
}

pub fn persist_first_seen<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset =
        initialize(&mut store).expect("store should initialize and we should get empty changeset");
    assert_eq!(changeset, ChangeSet::<ConfirmationBlockTime>::default());

    let tx1 = Arc::new(create_one_inp_one_out_tx(hash!("BTC"), 30_000));
    let tx2 = Arc::new(create_one_inp_one_out_tx(tx1.compute_txid(), 20_000));
    let tx3 = Arc::new(create_one_inp_one_out_tx(tx2.compute_txid(), 19_000));

    // try persisting and reading first_seen
    let txs: BTreeSet<Arc<Transaction>> = [tx1.clone(), tx2.clone()].into();
    let mut first_seen: BTreeMap<Txid, u64> = [
        (tx1.compute_txid(), 1755416600),
        (tx2.compute_txid(), 1755416600),
    ]
    .into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs,
        first_seen: first_seen.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };
    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.first_seen, first_seen);

    // persist another first_seen and see if what is read is same as merged one
    let txs_new: BTreeSet<Arc<Transaction>> = [tx3.clone()].into();
    let first_seen_new: BTreeMap<Txid, u64> = [(tx3.compute_txid(), 1755416700)].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs: txs_new,
        first_seen: first_seen_new.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };
    persist(&mut store, &changeset).expect("should persist changeset");

    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");
    first_seen.merge(first_seen_new);
    assert_eq!(changeset_read_new.first_seen, first_seen);
}

pub fn persist_txouts<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should initialize and load empty changeset");
    assert_eq!(changeset, ChangeSet::default());

    let mut txouts: BTreeMap<OutPoint, TxOut> = [
        (
            OutPoint::new(hash!("B"), 0),
            TxOut {
                value: Amount::from_sat(1300),
                script_pubkey: Address::from_str("bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl")
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
            },
        ),
        (
            OutPoint::new(hash!("D"), 0),
            TxOut {
                value: Amount::from_sat(1400),
                script_pubkey: Address::from_str("bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl")
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
            },
        ),
    ]
    .into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txouts: txouts.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");

    let changeset_read = initialize(&mut store).expect("should load changeset");
    assert_eq!(changeset_read.txouts, txouts);

    let txouts_new: BTreeMap<OutPoint, TxOut> = [(
        OutPoint::new(hash!("K"), 0),
        TxOut {
            value: Amount::from_sat(10000),
            script_pubkey: Address::from_str("bcrt1q8an5jfmpq8w2hr648nn34ecf9zdtxk0qyqtrfl")
                .unwrap()
                .assume_checked()
                .script_pubkey(),
        },
    )]
    .into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txouts: txouts_new.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");

    let changeset_read_new = initialize(&mut store).expect("should load changeset");
    txouts.merge(txouts_new);
    assert_eq!(changeset_read_new.txouts, txouts);
}

pub fn persist_txs<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should initialize and load empty changeset");
    assert_eq!(changeset, ChangeSet::<ConfirmationBlockTime>::default());

    let tx1 = Arc::new(create_one_inp_one_out_tx(hash!("BTC"), 30_000));
    let tx2 = Arc::new(create_one_inp_one_out_tx(tx1.compute_txid(), 20_000));
    let tx3 = Arc::new(create_one_inp_one_out_tx(tx2.compute_txid(), 19_000));

    let mut txs: BTreeSet<Arc<Transaction>> = [tx1, tx2.clone()].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs: txs.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.txs, txs);

    let txs_new: BTreeSet<Arc<Transaction>> = [tx3].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs: txs_new.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");
    txs.merge(txs_new);
    assert_eq!(changeset_read_new.txs, txs);
}

pub fn persist_anchors<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<tx_graph::ChangeSet<ConfirmationBlockTime>>,
    Persist: Fn(&mut Store, &tx_graph::ChangeSet<ConfirmationBlockTime>) -> anyhow::Result<()>,
{
    use tx_graph::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should initialize and load empty changeset");
    assert_eq!(changeset, ChangeSet::<ConfirmationBlockTime>::default());

    let tx1 = Arc::new(create_one_inp_one_out_tx(hash!(""), 30_000));
    let tx2 = Arc::new(create_one_inp_one_out_tx(tx1.compute_txid(), 20_000));
    let tx3 = Arc::new(create_one_inp_one_out_tx(tx2.compute_txid(), 19_000));

    let anchor1 = ConfirmationBlockTime {
        block_id: block_id!(23, "BTC"),
        confirmation_time: 1756838400,
    };

    let anchor2 = ConfirmationBlockTime {
        block_id: block_id!(25, "BDK"),
        confirmation_time: 1756839600,
    };

    let txs: BTreeSet<Arc<Transaction>> = [tx1.clone(), tx2.clone()].into();
    let mut anchors: BTreeSet<(ConfirmationBlockTime, Txid)> =
        [(anchor1, tx1.compute_txid()), (anchor2, tx2.compute_txid())].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs,
        anchors: anchors.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.anchors, anchors);

    let txs_new: BTreeSet<Arc<Transaction>> = [tx3.clone()].into();
    let anchors_new: BTreeSet<(ConfirmationBlockTime, Txid)> =
        [(anchor2, tx3.compute_txid())].into();

    let changeset = ChangeSet::<ConfirmationBlockTime> {
        txs: txs_new,
        anchors: anchors_new.clone(),
        ..ChangeSet::<ConfirmationBlockTime>::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");

    anchors.merge(anchors_new);
    assert_eq!(changeset_read.anchors, anchors);
}

// check the merge by changing asserts
pub fn persist_last_revealed<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<keychain_txout::ChangeSet>,
    Persist: Fn(&mut Store, &keychain_txout::ChangeSet) -> anyhow::Result<()>,
{
    use keychain_txout::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should initialize and load empty changeset");
    assert_eq!(changeset, ChangeSet::default());

    let descriptor_ids = crate::utils::DESCRIPTORS.map(|d| parse_descriptor(d).descriptor_id());

    let mut last_revealed: BTreeMap<DescriptorId, u32> =
        [(descriptor_ids[0], 1), (descriptor_ids[1], 100)].into();

    let changeset = ChangeSet {
        last_revealed: last_revealed.clone(),
        ..ChangeSet::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.last_revealed, last_revealed);

    let last_revealed_new: BTreeMap<DescriptorId, u32> = [(descriptor_ids[0], 2)].into();

    let changeset = ChangeSet {
        last_revealed: last_revealed_new.clone(),
        ..ChangeSet::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");
    last_revealed.merge(last_revealed_new);
    assert_eq!(changeset_read_new.last_revealed, last_revealed);
}

pub fn persist_spk_cache<Store, CreateStore, Initialize, Persist>(
    file_name: &str,
    create_store: CreateStore,
    initialize: Initialize,
    persist: Persist,
) where
    CreateStore: Fn(&Path) -> anyhow::Result<Store>,
    Initialize: Fn(&mut Store) -> anyhow::Result<keychain_txout::ChangeSet>,
    Persist: Fn(&mut Store, &keychain_txout::ChangeSet) -> anyhow::Result<()>,
{
    use keychain_txout::ChangeSet;
    let temp_dir = tempfile::tempdir().expect("must create tempdir");
    let file_path = temp_dir.path().join(file_name);
    let mut store = create_store(&file_path).expect("store should get created");

    let changeset = initialize(&mut store).expect("should initialize and load empty changeset");
    assert_eq!(changeset, ChangeSet::default());

    let descriptor_ids = crate::utils::DESCRIPTORS.map(|d| parse_descriptor(d).descriptor_id());
    let descs = crate::utils::DESCRIPTORS.map(parse_descriptor);

    let spk_cache: BTreeMap<DescriptorId, BTreeMap<u32, ScriptBuf>> = [
        (
            descriptor_ids[0],
            SpkIterator::new_with_range(&descs[0], 0..=125).collect(),
        ),
        (
            descriptor_ids[1],
            SpkIterator::new_with_range(&descs[0], 0..=25).collect(),
        ),
    ]
    .into();

    let changeset = ChangeSet {
        spk_cache: spk_cache.clone(),
        ..ChangeSet::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read = initialize(&mut store).expect("should load persisted changeset");
    assert_eq!(changeset_read.spk_cache, spk_cache);

    let spk_cache_new: BTreeMap<DescriptorId, BTreeMap<u32, ScriptBuf>> = [(
        descriptor_ids[0],
        SpkIterator::new_with_range(&descs[0], 126..=150).collect(),
    )]
    .into();

    let changeset = ChangeSet {
        spk_cache: spk_cache_new,
        ..ChangeSet::default()
    };

    persist(&mut store, &changeset).expect("should persist changeset");
    let changeset_read_new = initialize(&mut store).expect("should load persisted changeset");
    let spk_cache: BTreeMap<DescriptorId, BTreeMap<u32, ScriptBuf>> = [
        (
            descriptor_ids[0],
            SpkIterator::new_with_range(&descs[0], 0..=150).collect(),
        ),
        (
            descriptor_ids[1],
            SpkIterator::new_with_range(&descs[0], 0..=25).collect(),
        ),
    ]
    .into();
    assert_eq!(changeset_read_new.spk_cache, spk_cache);
}
