pub use anyhow;
use anyhow::Context;
use bdk_coin_select::{coin_select_bnb, CoinSelector, CoinSelectorOpt, WeightedValue};
use bdk_file_store::Store;
use serde::{de::DeserializeOwned, Serialize};
use std::{cmp::Reverse, collections::HashMap, path::PathBuf, sync::Mutex, time::Duration};

use bdk_chain::{
    bitcoin::{
        psbt::Prevouts, secp256k1::Secp256k1, util::sighash::SighashCache, Address, LockTime,
        Network, Sequence, Transaction, TxIn, TxOut,
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
pub type KeychainAdditions<A> = IndexedAdditions<A, DerivationAdditions<Keychain>>;
pub type Database<'m, C> = Persist<Store<'m, C>, C>;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
pub struct Args<S: clap::Subcommand> {
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
    pub command: Commands<S>,
}

#[allow(clippy::almost_swapped)]
#[derive(Subcommand, Debug, Clone)]
pub enum Commands<S: clap::Subcommand> {
    #[clap(flatten)]
    ChainSpecific(S),
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
        #[clap(short, default_value = "bnb")]
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

pub fn run_address_cmd<A, C>(
    graph: &mut KeychainTxGraph<A>,
    db: &Mutex<Database<C>>,
    network: Network,
    cmd: AddressCmd,
) -> anyhow::Result<()>
where
    C: Default + Append + DeserializeOwned + Serialize + From<KeychainAdditions<A>>,
{
    let index = &mut graph.index;

    match cmd {
        AddressCmd::Next | AddressCmd::New => {
            let spk_chooser = match cmd {
                AddressCmd::Next => KeychainTxOutIndex::next_unused_spk,
                AddressCmd::New => KeychainTxOutIndex::reveal_next_spk,
                _ => unreachable!("only these two variants exist in match arm"),
            };

            let ((spk_i, spk), index_additions) = spk_chooser(index, &Keychain::External);
            let db = &mut *db.lock().unwrap();
            db.stage(C::from(KeychainAdditions::from(index_additions)));
            db.commit()?;
            let addr = Address::from_script(spk, network).context("failed to derive address")?;
            println!("[address @ {}] {}", spk_i, addr);
            Ok(())
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
    fn print_balances<'a>(title_str: &'a str, items: impl IntoIterator<Item = (&'a str, u64)>) {
        println!("{}:", title_str);
        for (name, amount) in items.into_iter() {
            println!("    {:<10} {:>12} sats", name, amount)
        }
    }

    let balance = graph.graph().try_balance(
        chain,
        chain.get_chain_tip()?.unwrap_or_default(),
        graph.index.outpoints().iter().cloned(),
        |(k, _), _| k == &Keychain::Internal,
    )?;

    let confirmed_total = balance.confirmed + balance.immature;
    let unconfirmed_total = balance.untrusted_pending + balance.trusted_pending;

    print_balances(
        "confirmed",
        [
            ("total", confirmed_total),
            ("spendable", balance.confirmed),
            ("immature", balance.immature),
        ],
    );
    print_balances(
        "unconfirmed",
        [
            ("total", unconfirmed_total),
            ("trusted", balance.trusted_pending),
            ("untrusted", balance.untrusted_pending),
        ],
    );

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
pub fn run_send_cmd<A: Anchor, O: ChainOracle, C>(
    graph: &Mutex<KeychainTxGraph<A>>,
    db: &Mutex<Database<'_, C>>,
    chain: &O,
    keymap: &HashMap<DescriptorPublicKey, DescriptorSecretKey>,
    cs_algorithm: CoinSelectionAlgo,
    address: Address,
    value: u64,
    broadcast: impl FnOnce(&Transaction) -> anyhow::Result<()>,
) -> anyhow::Result<()>
where
    O::Error: std::error::Error + Send + Sync + 'static,
    C: Default + Append + DeserializeOwned + Serialize + From<KeychainAdditions<A>>,
{
    let (transaction, change_index) = {
        let graph = &mut *graph.lock().unwrap();
        // take mutable ref to construct tx -- it is only open for a short time while building it.
        let (tx, change_info) = create_tx(graph, chain, keymap, cs_algorithm, address, value)?;

        if let Some((index_additions, (change_keychain, index))) = change_info {
            // We must first persist to disk the fact that we've got a new address from the
            // change keychain so future scans will find the tx we're about to broadcast.
            // If we're unable to persist this, then we don't want to broadcast.
            {
                let db = &mut *db.lock().unwrap();
                db.stage(C::from(KeychainAdditions::from(index_additions)));
                db.commit()?;
            }

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

            let keychain_additions = graph.lock().unwrap().insert_tx(&transaction, None, None);

            // We know the tx is at least unconfirmed now. Note if persisting here fails,
            // it's not a big deal since we can always find it again form
            // blockchain.
            db.lock().unwrap().stage(C::from(keychain_additions));
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
        // because the temporary planning module does not support timelocks, we can use the chain
        // tip as the `lock_time` for anti-fee-sniping purposes
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

pub fn handle_commands<S: clap::Subcommand, A: Anchor, O: ChainOracle, C>(
    graph: &Mutex<KeychainTxGraph<A>>,
    db: &Mutex<Database<C>>,
    chain: &Mutex<O>,
    keymap: &HashMap<DescriptorPublicKey, DescriptorSecretKey>,
    network: Network,
    broadcast: impl FnOnce(&Transaction) -> anyhow::Result<()>,
    cmd: Commands<S>,
) -> anyhow::Result<()>
where
    O::Error: std::error::Error + Send + Sync + 'static,
    C: Default + Append + DeserializeOwned + Serialize + From<KeychainAdditions<A>>,
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

#[allow(clippy::type_complexity)]
pub fn init<'m, S: clap::Subcommand, C>(
    db_magic: &'m [u8],
    db_default_path: &str,
) -> anyhow::Result<(
    Args<S>,
    KeyMap,
    KeychainTxOutIndex<Keychain>,
    Mutex<Database<'m, C>>,
    C,
)>
where
    C: Default + Append + Serialize + DeserializeOwned,
{
    if std::env::var("BDK_DB_PATH").is_err() {
        std::env::set_var("BDK_DB_PATH", db_default_path);
    }
    let args = Args::<S>::parse();
    let secp = Secp256k1::default();

    let mut index = KeychainTxOutIndex::<Keychain>::default();

    let (descriptor, mut keymap) =
        Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &args.descriptor)?;
    index.add_keychain(Keychain::External, descriptor);

    if let Some((internal_descriptor, internal_keymap)) = args
        .change_descriptor
        .as_ref()
        .map(|desc_str| Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, desc_str))
        .transpose()?
    {
        keymap.extend(internal_keymap);
        index.add_keychain(Keychain::Internal, internal_descriptor);
    }

    let mut db_backend = match Store::<'m, C>::new_from_path(db_magic, &args.db_path) {
        Ok(db_backend) => db_backend,
        // we cannot return `err` directly as it has lifetime `'m`
        Err(err) => return Err(anyhow::anyhow!("failed to init db backend: {:?}", err)),
    };

    let init_changeset = db_backend.load_from_persistence()?;

    Ok((
        args,
        keymap,
        index,
        Mutex::new(Database::new(db_backend)),
        init_changeset,
    ))
}
