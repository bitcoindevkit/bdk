pub use anyhow;
use anyhow::Context;
use bdk_coin_select::{coin_select_bnb, CoinSelector, CoinSelectorOpt, WeightedValue};
use bdk_file_store::Store;
use serde::{de::DeserializeOwned, Serialize};
use std::{cmp::Reverse, collections::HashMap, path::PathBuf, sync::Mutex, time::Duration};

use bdk_chain::{
    bitcoin::{
        psbt::Prevouts,
        secp256k1::{self, Secp256k1},
        util::sighash::SighashCache,
        Address, LockTime, Network, Script, Sequence, Transaction, TxIn, TxOut,
    },
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{DerivationAdditions, KeychainTxOutIndex},
    miniscript::{
        descriptor::{DescriptorSecretKey, KeyMap},
        Descriptor, DescriptorPublicKey,
    },
    Anchor, Append, ChainOracle, DescriptorExt, FullTxOut, ObservedAs, Persist, PersistBackend,
};
pub use bdk_file_store;
pub use clap;

use clap::{Parser, Subcommand};

pub type KeychainTxGraph<A> = IndexedTxGraph<A, KeychainTxOutIndex<Keychain>>;
pub type Database<'m, A, X> = Persist<Store<'m, ChangeSet<A, X>>, ChangeSet<A, X>>;

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(bound(
    deserialize = "A: Ord + serde::Deserialize<'de>, X: serde::Deserialize<'de>",
    serialize = "A: Ord + serde::Serialize, X: serde::Serialize",
))]
pub struct ChangeSet<A, X> {
    pub indexed_additions: IndexedAdditions<A, DerivationAdditions<Keychain>>,
    pub extension: X,
}

impl<A, X: Default> Default for ChangeSet<A, X> {
    fn default() -> Self {
        Self {
            indexed_additions: Default::default(),
            extension: Default::default(),
        }
    }
}

impl<A: Anchor, X: Append> Append for ChangeSet<A, X> {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.indexed_additions, other.indexed_additions);
        Append::append(&mut self.extension, other.extension)
    }

    fn is_empty(&self) -> bool {
        self.indexed_additions.is_empty() && self.extension.is_empty()
    }
}

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
pub struct Args<C: clap::Subcommand> {
    #[clap(env = "DESCRIPTOR")]
    pub descriptor: String,
    #[clap(env = "CHANGE_DESCRIPTOR")]
    pub change_descriptor: Option<String>,

    #[clap(env = "BITCOIN_NETWORK", long, default_value = "signet")]
    pub network: Network,

    #[clap(env = "BDK_DB_PATH", long, default_value = ".bdk_example_db")]
    pub db_path: PathBuf,

    #[clap(env = "BDK_CP_LIMIT", long, default_value = "20")]
    pub cp_limit: usize,

    #[clap(subcommand)]
    pub command: Commands<C>,
}

#[allow(clippy::almost_swapped)]
#[derive(Subcommand, Debug, Clone)]
pub enum Commands<C: clap::Subcommand> {
    #[clap(flatten)]
    ChainSpecific(C),
    /// Address generation and inspection.
    Address {
        #[clap(subcommand)]
        addr_cmd: AddressCmd,
    },
    /// Get the wallet balance.
    Balance,
    /// TxOut related commands.
    #[clap(name = "txout")]
    TxOut {
        #[clap(subcommand)]
        txout_cmd: TxOutCmd,
    },
    /// Send coins to an address.
    Send {
        value: u64,
        address: Address,
        #[clap(short, default_value = "largest-first")]
        coin_select: CoinSelectionAlgo,
    },
}

#[derive(Clone, Debug)]
pub enum CoinSelectionAlgo {
    LargestFirst,
    SmallestFirst,
    OldestFirst,
    NewestFirst,
    BranchAndBound,
}

impl Default for CoinSelectionAlgo {
    fn default() -> Self {
        Self::LargestFirst
    }
}

impl core::str::FromStr for CoinSelectionAlgo {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use CoinSelectionAlgo::*;
        Ok(match s {
            "largest-first" => LargestFirst,
            "smallest-first" => SmallestFirst,
            "oldest-first" => OldestFirst,
            "newest-first" => NewestFirst,
            "bnb" => BranchAndBound,
            unknown => {
                return Err(anyhow::anyhow!(
                    "unknown coin selection algorithm '{}'",
                    unknown
                ))
            }
        })
    }
}

impl core::fmt::Display for CoinSelectionAlgo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use CoinSelectionAlgo::*;
        write!(
            f,
            "{}",
            match self {
                LargestFirst => "largest-first",
                SmallestFirst => "smallest-first",
                OldestFirst => "oldest-first",
                NewestFirst => "newest-first",
                BranchAndBound => "bnb",
            }
        )
    }
}

#[allow(clippy::almost_swapped)]
#[derive(Subcommand, Debug, Clone)]
pub enum AddressCmd {
    /// Get the next unused address.
    Next,
    /// Get a new address regardless of the existing unused addresses.
    New,
    /// List all addresses
    List {
        #[clap(long)]
        change: bool,
    },
    Index,
}

#[derive(Subcommand, Debug, Clone)]
pub enum TxOutCmd {
    List {
        /// Return only spent outputs.
        #[clap(short, long)]
        spent: bool,
        /// Return only unspent outputs.
        #[clap(short, long)]
        unspent: bool,
        /// Return only confirmed outputs.
        #[clap(long)]
        confirmed: bool,
        /// Return only unconfirmed outputs.
        #[clap(long)]
        unconfirmed: bool,
    },
}

#[derive(
    Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, serde::Deserialize, serde::Serialize,
)]
pub enum Keychain {
    External,
    Internal,
}

impl core::fmt::Display for Keychain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Keychain::External => write!(f, "external"),
            Keychain::Internal => write!(f, "internal"),
        }
    }
}

pub fn run_address_cmd<A, X>(
    graph: &mut KeychainTxGraph<A>,
    db: &Mutex<Database<'_, A, X>>,
    network: Network,
    cmd: AddressCmd,
) -> anyhow::Result<()>
where
    ChangeSet<A, X>: Default + Append + DeserializeOwned + Serialize,
{
    let process_spk = |spk_i: u32, spk: &Script, index_additions: DerivationAdditions<Keychain>| {
        if !index_additions.is_empty() {
            let db = &mut *db.lock().unwrap();
            db.stage(ChangeSet {
                indexed_additions: IndexedAdditions {
                    index_additions,
                    ..Default::default()
                },
                ..Default::default()
            });
            db.commit()?;
        }
        let addr = Address::from_script(spk, network).context("failed to derive address")?;
        println!("[address @ {}] {}", spk_i, addr);
        Ok(())
    };

    let index = &mut graph.index;

    match cmd {
        AddressCmd::Next => {
            let ((spk_i, spk), index_additions) = index.next_unused_spk(&Keychain::External);
            process_spk(spk_i, spk, index_additions)
        }
        AddressCmd::New => {
            let ((spk_i, spk), index_additions) = index.reveal_next_spk(&Keychain::External);
            process_spk(spk_i, spk, index_additions)
        }
        AddressCmd::Index => {
            for (keychain, derivation_index) in index.last_revealed_indices() {
                println!("{:?}: {}", keychain, derivation_index);
            }
            Ok(())
        }
        AddressCmd::List { change } => {
            let target_keychain = match change {
                true => Keychain::Internal,
                false => Keychain::External,
            };
            for (spk_i, spk) in index.revealed_spks_of_keychain(&target_keychain) {
                let address = Address::from_script(spk, network)
                    .expect("should always be able to derive address");
                println!(
                    "{:?} {} used:{}",
                    spk_i,
                    address,
                    index.is_used(&(target_keychain, spk_i))
                );
            }
            Ok(())
        }
    }
}

pub fn run_balance_cmd<A: Anchor, O: ChainOracle>(
    graph: &KeychainTxGraph<A>,
    chain: &O,
) -> Result<(), O::Error> {
    let balance = graph.graph().try_balance(
        chain,
        chain.get_chain_tip()?.unwrap_or_default(),
        graph.index.outpoints().iter().cloned(),
        |(k, _), _| k == &Keychain::Internal,
    )?;

    let confirmed_total = balance.confirmed + balance.immature;
    let unconfirmed_total = balance.untrusted_pending + balance.trusted_pending;

    println!("[confirmed]");
    println!("  total     = {}sats", confirmed_total);
    println!("  spendable = {}sats", balance.confirmed);
    println!("  immature  = {}sats", balance.immature);

    println!("[unconfirmed]");
    println!("  total     = {}sats", unconfirmed_total,);
    println!("  trusted   = {}sats", balance.trusted_pending);
    println!("  untrusted = {}sats", balance.untrusted_pending);

    Ok(())
}

pub fn run_txo_cmd<A: Anchor, O: ChainOracle>(
    graph: &KeychainTxGraph<A>,
    chain: &O,
    network: Network,
    cmd: TxOutCmd,
) -> anyhow::Result<()>
where
    O::Error: std::error::Error + Send + Sync + 'static,
{
    let chain_tip = chain.get_chain_tip()?.unwrap_or_default();
    let outpoints = graph.index.outpoints().iter().cloned();

    match cmd {
        TxOutCmd::List {
            spent,
            unspent,
            confirmed,
            unconfirmed,
        } => {
            let txouts = graph
                .graph()
                .try_filter_chain_txouts(chain, chain_tip, outpoints)
                .filter(|r| match r {
                    Ok((_, full_txo)) => match (spent, unspent) {
                        (true, false) => full_txo.spent_by.is_some(),
                        (false, true) => full_txo.spent_by.is_none(),
                        _ => true,
                    },
                    // always keep errored items
                    Err(_) => true,
                })
                .filter(|r| match r {
                    Ok((_, full_txo)) => match (confirmed, unconfirmed) {
                        (true, false) => full_txo.chain_position.is_confirmed(),
                        (false, true) => !full_txo.chain_position.is_confirmed(),
                        _ => true,
                    },
                    // always keep errored items
                    Err(_) => true,
                })
                .collect::<Result<Vec<_>, _>>()?;

            for (spk_i, full_txo) in txouts {
                let addr = Address::from_script(&full_txo.txout.script_pubkey, network)?;
                println!(
                    "{:?} {} {} {} spent:{:?}",
                    spk_i, full_txo.txout.value, full_txo.outpoint, addr, full_txo.spent_by
                )
            }
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_send_cmd<A: Anchor, O: ChainOracle, X>(
    graph: &Mutex<KeychainTxGraph<A>>,
    db: &Mutex<Database<'_, A, X>>,
    chain: &O,
    keymap: &HashMap<DescriptorPublicKey, DescriptorSecretKey>,
    cs_algorithm: CoinSelectionAlgo,
    address: Address,
    value: u64,
    broadcast: impl FnOnce(&Transaction) -> anyhow::Result<()>,
) -> anyhow::Result<()>
where
    O::Error: std::error::Error + Send + Sync + 'static,
    ChangeSet<A, X>: Default + Append + DeserializeOwned + Serialize,
{
    let (transaction, change_index) = {
        let graph = &mut *graph.lock().unwrap();
        // take mutable ref to construct tx -- it is only open for a short time while building it.
        let (tx, change_info) = create_tx(graph, chain, keymap, cs_algorithm, address, value)?;

        if let Some((index_additions, (change_keychain, index))) = change_info {
            // We must first persist to disk the fact that we've got a new address from the
            // change keychain so future scans will find the tx we're about to broadcast.
            // If we're unable to persist this, then we don't want to broadcast.
            db.lock().unwrap().stage(ChangeSet {
                indexed_additions: IndexedAdditions {
                    index_additions,
                    ..Default::default()
                },
                ..Default::default()
            });

            // We don't want other callers/threads to use this address while we're using it
            // but we also don't want to scan the tx we just created because it's not
            // technically in the blockchain yet.
            graph.index.mark_used(&change_keychain, index);
            (tx, Some((change_keychain, index)))
        } else {
            (tx, None)
        }
    };

    match (broadcast)(&transaction) {
        Ok(_) => {
            println!("Broadcasted Tx : {}", transaction.txid());

            let indexed_additions = graph.lock().unwrap().insert_tx(&transaction, None, None);

            // We know the tx is at least unconfirmed now. Note if persisting here fails,
            // it's not a big deal since we can always find it again form
            // blockchain.
            db.lock().unwrap().stage(ChangeSet {
                indexed_additions,
                ..Default::default()
            });
            Ok(())
        }
        Err(e) => {
            if let Some((keychain, index)) = change_index {
                // We failed to broadcast, so allow our change address to be used in the future
                graph.lock().unwrap().index.unmark_used(&keychain, index);
            }
            Err(e)
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn create_tx<A: Anchor, O: ChainOracle>(
    graph: &mut KeychainTxGraph<A>,
    chain: &O,
    keymap: &HashMap<DescriptorPublicKey, DescriptorSecretKey>,
    cs_algorithm: CoinSelectionAlgo,
    address: Address,
    value: u64,
) -> anyhow::Result<(
    Transaction,
    Option<(DerivationAdditions<Keychain>, (Keychain, u32))>,
)>
where
    O::Error: std::error::Error + Send + Sync + 'static,
{
    let mut additions = DerivationAdditions::default();

    let assets = bdk_tmp_plan::Assets {
        keys: keymap.iter().map(|(pk, _)| pk.clone()).collect(),
        ..Default::default()
    };

    // TODO use planning module
    let mut candidates = planned_utxos(graph, chain, &assets)?;

    // apply coin selection algorithm
    match cs_algorithm {
        CoinSelectionAlgo::LargestFirst => {
            candidates.sort_by_key(|(_, utxo)| Reverse(utxo.txout.value))
        }
        CoinSelectionAlgo::SmallestFirst => candidates.sort_by_key(|(_, utxo)| utxo.txout.value),
        CoinSelectionAlgo::OldestFirst => {
            candidates.sort_by_key(|(_, utxo)| utxo.chain_position.clone())
        }
        CoinSelectionAlgo::NewestFirst => {
            candidates.sort_by_key(|(_, utxo)| Reverse(utxo.chain_position.clone()))
        }
        CoinSelectionAlgo::BranchAndBound => {}
    }

    // turn the txos we chose into weight and value
    let wv_candidates = candidates
        .iter()
        .map(|(plan, utxo)| {
            WeightedValue::new(
                utxo.txout.value,
                plan.expected_weight() as _,
                plan.witness_version().is_some(),
            )
        })
        .collect();

    let mut outputs = vec![TxOut {
        value,
        script_pubkey: address.script_pubkey(),
    }];

    let internal_keychain = if graph.index.keychains().get(&Keychain::Internal).is_some() {
        Keychain::Internal
    } else {
        Keychain::External
    };

    let ((change_index, change_script), change_additions) =
        graph.index.next_unused_spk(&internal_keychain);
    additions.append(change_additions);

    // Clone to drop the immutable reference.
    let change_script = change_script.clone();

    let change_plan = bdk_tmp_plan::plan_satisfaction(
        &graph
            .index
            .keychains()
            .get(&internal_keychain)
            .expect("must exist")
            .at_derivation_index(change_index),
        &assets,
    )
    .expect("failed to obtain change plan");

    let mut change_output = TxOut {
        value: 0,
        script_pubkey: change_script,
    };

    let cs_opts = CoinSelectorOpt {
        target_feerate: 0.5,
        min_drain_value: graph
            .index
            .keychains()
            .get(&internal_keychain)
            .expect("must exist")
            .dust_value(),
        ..CoinSelectorOpt::fund_outputs(
            &outputs,
            &change_output,
            change_plan.expected_weight() as u32,
        )
    };

    // TODO: How can we make it easy to shuffle in order of inputs and outputs here?
    // apply coin selection by saying we need to fund these outputs
    let mut coin_selector = CoinSelector::new(&wv_candidates, &cs_opts);

    // just select coins in the order provided until we have enough
    // only use the first result (least waste)
    let selection = match cs_algorithm {
        CoinSelectionAlgo::BranchAndBound => {
            coin_select_bnb(Duration::from_secs(10), coin_selector.clone())
                .map_or_else(|| coin_selector.select_until_finished(), |cs| cs.finish())?
        }
        _ => coin_selector.select_until_finished()?,
    };
    let (_, selection_meta) = selection.best_strategy();

    // get the selected utxos
    let selected_txos = selection.apply_selection(&candidates).collect::<Vec<_>>();

    if let Some(drain_value) = selection_meta.drain_value {
        change_output.value = drain_value;
        // if the selection tells us to use change and the change value is sufficient, we add it as an output
        outputs.push(change_output)
    }

    let mut transaction = Transaction {
        version: 0x02,
        lock_time: chain
            .get_chain_tip()?
            .and_then(|block_id| LockTime::from_height(block_id.height).ok())
            .unwrap_or(LockTime::ZERO)
            .into(),
        input: selected_txos
            .iter()
            .map(|(_, utxo)| TxIn {
                previous_output: utxo.outpoint,
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..Default::default()
            })
            .collect(),
        output: outputs,
    };

    let prevouts = selected_txos
        .iter()
        .map(|(_, utxo)| utxo.txout.clone())
        .collect::<Vec<_>>();
    let sighash_prevouts = Prevouts::All(&prevouts);

    // first, set tx values for the plan so that we don't change them while signing
    for (i, (plan, _)) in selected_txos.iter().enumerate() {
        if let Some(sequence) = plan.required_sequence() {
            transaction.input[i].sequence = sequence
        }
    }

    // create a short lived transaction
    let _sighash_tx = transaction.clone();
    let mut sighash_cache = SighashCache::new(&_sighash_tx);

    for (i, (plan, _)) in selected_txos.iter().enumerate() {
        let requirements = plan.requirements();
        let mut auth_data = bdk_tmp_plan::SatisfactionMaterial::default();
        assert!(
            !requirements.requires_hash_preimages(),
            "can't have hash pre-images since we didn't provide any."
        );
        assert!(
            requirements.signatures.sign_with_keymap(
                i,
                keymap,
                &sighash_prevouts,
                None,
                None,
                &mut sighash_cache,
                &mut auth_data,
                &Secp256k1::default(),
            )?,
            "we should have signed with this input."
        );

        match plan.try_complete(&auth_data) {
            bdk_tmp_plan::PlanState::Complete {
                final_script_sig,
                final_script_witness,
            } => {
                if let Some(witness) = final_script_witness {
                    transaction.input[i].witness = witness;
                }

                if let Some(script_sig) = final_script_sig {
                    transaction.input[i].script_sig = script_sig;
                }
            }
            bdk_tmp_plan::PlanState::Incomplete(_) => {
                return Err(anyhow::anyhow!(
                    "we weren't able to complete the plan with our keys."
                ));
            }
        }
    }

    let change_info = if selection_meta.drain_value.is_some() {
        Some((additions, (internal_keychain, change_index)))
    } else {
        None
    };

    Ok((transaction, change_info))
}

#[allow(clippy::type_complexity)]
pub fn planned_utxos<A: Anchor, O: ChainOracle, K: Clone + bdk_tmp_plan::CanDerive>(
    graph: &KeychainTxGraph<A>,
    chain: &O,
    assets: &bdk_tmp_plan::Assets<K>,
) -> Result<Vec<(bdk_tmp_plan::Plan<K>, FullTxOut<ObservedAs<A>>)>, O::Error> {
    let chain_tip = chain.get_chain_tip()?.unwrap_or_default();
    let outpoints = graph.index.outpoints().iter().cloned();
    graph
        .graph()
        .try_filter_chain_unspents(chain, chain_tip, outpoints)
        .filter_map(
            #[allow(clippy::type_complexity)]
            |r| -> Option<Result<(bdk_tmp_plan::Plan<K>, FullTxOut<ObservedAs<A>>), _>> {
                let (k, i, full_txo) = match r {
                    Err(err) => return Some(Err(err)),
                    Ok(((k, i), full_txo)) => (k, i, full_txo),
                };
                let desc = graph
                    .index
                    .keychains()
                    .get(&k)
                    .expect("keychain must exist")
                    .at_derivation_index(i);
                let plan = bdk_tmp_plan::plan_satisfaction(&desc, assets)?;
                Some(Ok((plan, full_txo)))
            },
        )
        .collect()
}

pub fn handle_commands<C: clap::Subcommand, A: Anchor, O: ChainOracle, X>(
    graph: &Mutex<KeychainTxGraph<A>>,
    db: &Mutex<Database<A, X>>,
    chain: &Mutex<O>,
    keymap: &HashMap<DescriptorPublicKey, DescriptorSecretKey>,
    network: Network,
    broadcast: impl FnOnce(&Transaction) -> anyhow::Result<()>,
    cmd: Commands<C>,
) -> anyhow::Result<()>
where
    O::Error: std::error::Error + Send + Sync + 'static,
    ChangeSet<A, X>: Default + Append + DeserializeOwned + Serialize,
{
    match cmd {
        Commands::ChainSpecific(_) => unreachable!("example code should handle this!"),
        Commands::Address { addr_cmd } => {
            let graph = &mut *graph.lock().unwrap();
            run_address_cmd(graph, db, network, addr_cmd)
        }
        Commands::Balance => {
            let graph = &*graph.lock().unwrap();
            let chain = &*chain.lock().unwrap();
            run_balance_cmd(graph, chain).map_err(anyhow::Error::from)
        }
        Commands::TxOut { txout_cmd } => {
            let graph = &*graph.lock().unwrap();
            let chain = &*chain.lock().unwrap();
            run_txo_cmd(graph, chain, network, txout_cmd)
        }
        Commands::Send {
            value,
            address,
            coin_select,
        } => {
            let chain = &*chain.lock().unwrap();
            run_send_cmd(
                graph,
                db,
                chain,
                keymap,
                coin_select,
                address,
                value,
                broadcast,
            )
        }
    }
}

pub fn prepare_index<C: clap::Subcommand, SC: secp256k1::Signing>(
    args: &Args<C>,
    secp: &Secp256k1<SC>,
) -> anyhow::Result<(KeychainTxOutIndex<Keychain>, KeyMap)> {
    let mut index = KeychainTxOutIndex::<Keychain>::default();

    let (descriptor, mut keymap) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(secp, &args.descriptor)?;
    index.add_keychain(Keychain::External, descriptor);

    if let Some((internal_descriptor, internal_keymap)) = args
        .change_descriptor
        .as_ref()
        .map(|desc_str| Descriptor::<DescriptorPublicKey>::parse_descriptor(secp, desc_str))
        .transpose()?
    {
        keymap.extend(internal_keymap);
        index.add_keychain(Keychain::Internal, internal_descriptor);
    }

    Ok((index, keymap))
}

#[allow(clippy::type_complexity)]
pub fn init<'m, S: clap::Subcommand, A: Anchor, X>(
    db_magic: &'m [u8],
    db_default_path: &str,
) -> anyhow::Result<(
    Args<S>,
    KeyMap,
    Mutex<KeychainTxGraph<A>>,
    Mutex<Database<'m, A, X>>,
    X,
)>
where
    ChangeSet<A, X>: Default + Append + Serialize + DeserializeOwned,
{
    if std::env::var("BDK_DB_PATH").is_err() {
        std::env::set_var("BDK_DB_PATH", db_default_path);
    }
    let args = Args::<S>::parse();
    let secp = Secp256k1::default();
    let (index, keymap) = prepare_index(&args, &secp)?;

    let mut indexed_graph = IndexedTxGraph::<A, KeychainTxOutIndex<Keychain>>::new(index);

    let mut db_backend =
        match Store::<'m, ChangeSet<A, X>>::new_from_path(db_magic, args.db_path.as_path()) {
            Ok(db_backend) => db_backend,
            Err(err) => return Err(anyhow::anyhow!("failed to init db backend: {:?}", err)),
        };

    let ChangeSet {
        indexed_additions,
        extension,
    } = db_backend.load_from_persistence()?;
    indexed_graph.apply_additions(indexed_additions);

    Ok((
        args,
        keymap,
        Mutex::new(indexed_graph),
        Mutex::new(Database::new(db_backend)),
        extension,
    ))
}
