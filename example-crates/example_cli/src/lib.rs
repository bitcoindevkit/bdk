pub use anyhow;
use anyhow::Context;
use bdk_coin_select::{Candidate, CoinSelector};
use bdk_file_store::Store;
use serde::{de::DeserializeOwned, Serialize};
use std::{cmp::Reverse, collections::HashMap, path::PathBuf, sync::Mutex};

use bdk_chain::{
    bitcoin::{
        absolute, address, psbt::Prevouts, secp256k1::Secp256k1, sighash::SighashCache, Address,
        Network, Sequence, Transaction, TxIn, TxOut,
    },
    indexed_tx_graph::{self, IndexedTxGraph},
    keychain::{self, KeychainTxOutIndex},
    local_chain,
    miniscript::{
        descriptor::{DescriptorSecretKey, KeyMap},
        Descriptor, DescriptorPublicKey,
    },
    Anchor, Append, ChainOracle, FullTxOut, Persist, PersistBackend,
};
pub use bdk_file_store;
pub use clap;

use clap::{Parser, Subcommand};

pub type KeychainTxGraph<A> = IndexedTxGraph<A, KeychainTxOutIndex<Keychain>>;
pub type KeychainChangeSet<A> = (
    local_chain::ChangeSet,
    indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<Keychain>>,
);
pub type Database<'m, C> = Persist<Store<'m, C>, C>;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
pub struct Args<CS: clap::Subcommand, S: clap::Args> {
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
    pub command: Commands<CS, S>,
}

#[allow(clippy::almost_swapped)]
#[derive(Subcommand, Debug, Clone)]
pub enum Commands<CS: clap::Subcommand, S: clap::Args> {
    #[clap(flatten)]
    ChainSpecific(CS),
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
        address: Address<address::NetworkUnchecked>,
        #[clap(short, default_value = "bnb")]
        coin_select: CoinSelectionAlgo,
        #[clap(flatten)]
        chain_specfic: S,
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
    Option<(keychain::ChangeSet<Keychain>, (Keychain, u32))>,
)>
where
    O::Error: std::error::Error + Send + Sync + 'static,
{
    let mut changeset = keychain::ChangeSet::default();

    let assets = bdk_tmp_plan::Assets {
        keys: keymap.iter().map(|(pk, _)| pk.clone()).collect(),
        ..Default::default()
    };

    // TODO use planning module
    let raw_candidates = planned_utxos(graph, chain, &assets)?;
    // turn the txos we chose into weight and value
    let candidates = raw_candidates
        .iter()
        .map(|(plan, utxo)| {
            Candidate::new(
                utxo.txout.value,
                plan.expected_weight() as _,
                plan.witness_version().is_some(),
            )
        })
        .collect::<Vec<_>>();

    let internal_keychain = if graph.index.keychains().get(&Keychain::Internal).is_some() {
        Keychain::Internal
    } else {
        Keychain::External
    };

    let ((change_index, change_script), change_changeset) =
        graph.index.next_unused_spk(&internal_keychain);
    changeset.append(change_changeset);

    // Clone to drop the immutable reference.
    let change_script = change_script.to_owned();

    let change_plan = bdk_tmp_plan::plan_satisfaction(
        &graph
            .index
            .keychains()
            .get(&internal_keychain)
            .expect("must exist")
            .at_derivation_index(change_index)
            .expect("change_index can't be hardened"),
        &assets,
    )
    .expect("failed to obtain change plan");

    let mut transaction = Transaction {
        version: 0x02,
        // because the temporary planning module does not support timelocks, we can use the chain
        // tip as the `lock_time` for anti-fee-sniping purposes
        lock_time: chain
            .get_chain_tip()?
            .and_then(|block_id| absolute::LockTime::from_height(block_id.height).ok())
            .unwrap_or(absolute::LockTime::ZERO),
        input: vec![],
        output: vec![TxOut {
            value,
            script_pubkey: address.script_pubkey(),
        }],
    };

    let target = bdk_coin_select::Target {
        feerate: bdk_coin_select::FeeRate::from_sat_per_vb(2.0),
        min_fee: 0,
        value: transaction.output.iter().map(|txo| txo.value).sum(),
    };

    let drain_weights = bdk_coin_select::DrainWeights {
        output_weight: {
            // we calculate the weight difference of including the drain output in the base tx
            // this method will detect varint size changes of txout count
            let tx_weight = transaction.weight();
            let tx_weight_with_drain = {
                let mut tx = transaction.clone();
                tx.output.push(TxOut {
                    script_pubkey: change_script.clone(),
                    ..Default::default()
                });
                tx.weight()
            };
            (tx_weight_with_drain - tx_weight).to_wu() as u32 - 1
        },
        spend_weight: change_plan.expected_weight() as u32,
    };
    let long_term_feerate = bdk_coin_select::FeeRate::from_sat_per_vb(5.0);
    let drain_policy = bdk_coin_select::change_policy::min_value_and_waste(
        drain_weights,
        change_script.dust_value().to_sat(),
        long_term_feerate,
    );

    let mut selector = CoinSelector::new(&candidates, transaction.weight().to_wu() as u32);
    match cs_algorithm {
        CoinSelectionAlgo::BranchAndBound => {
            let metric = bdk_coin_select::metrics::Waste {
                target,
                long_term_feerate,
                change_policy: &drain_policy,
            };
            if let Err(bnb_err) = selector.run_bnb(metric, 100_000) {
                selector.sort_candidates_by_descending_value_pwu();
                println!(
                    "Error: {} Falling back to select until target met.",
                    bnb_err
                );
            };
        }
        CoinSelectionAlgo::LargestFirst => {
            selector.sort_candidates_by_key(|(_, c)| Reverse(c.value))
        }
        CoinSelectionAlgo::SmallestFirst => selector.sort_candidates_by_key(|(_, c)| c.value),
        CoinSelectionAlgo::OldestFirst => {
            selector.sort_candidates_by_key(|(i, _)| raw_candidates[i].1.chain_position.clone())
        }
        CoinSelectionAlgo::NewestFirst => selector
            .sort_candidates_by_key(|(i, _)| Reverse(raw_candidates[i].1.chain_position.clone())),
    };

    // ensure target is met
    selector.select_until_target_met(target, drain_policy(&selector, target))?;

    // get the selected utxos
    let selected_txos = selector
        .apply_selection(&raw_candidates)
        .collect::<Vec<_>>();

    let drain = drain_policy(&selector, target);
    if drain.is_some() {
        transaction.output.push(TxOut {
            value: drain.value,
            script_pubkey: change_script,
        });
    }

    // fill transaction inputs
    transaction.input = selected_txos
        .iter()
        .map(|(_, utxo)| TxIn {
            previous_output: utxo.outpoint,
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            ..Default::default()
        })
        .collect();

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

    let change_info = if drain.is_some() {
        Some((changeset, (internal_keychain, change_index)))
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
) -> Result<Vec<(bdk_tmp_plan::Plan<K>, FullTxOut<A>)>, O::Error> {
    let chain_tip = chain.get_chain_tip()?.unwrap_or_default();
    let outpoints = graph.index.outpoints().iter().cloned();
    graph
        .graph()
        .try_filter_chain_unspents(chain, chain_tip, outpoints)
        .filter_map(
            #[allow(clippy::type_complexity)]
            |r| -> Option<Result<(bdk_tmp_plan::Plan<K>, FullTxOut<A>), _>> {
                let (k, i, full_txo) = match r {
                    Err(err) => return Some(Err(err)),
                    Ok(((k, i), full_txo)) => (k, i, full_txo),
                };
                let desc = graph
                    .index
                    .keychains()
                    .get(&k)
                    .expect("keychain must exist")
                    .at_derivation_index(i)
                    .expect("i can't be hardened");
                let plan = bdk_tmp_plan::plan_satisfaction(&desc, assets)?;
                Some(Ok((plan, full_txo)))
            },
        )
        .collect()
}

pub fn handle_commands<CS: clap::Subcommand, S: clap::Args, A: Anchor, O: ChainOracle, C>(
    graph: &Mutex<KeychainTxGraph<A>>,
    db: &Mutex<Database<C>>,
    chain: &Mutex<O>,
    keymap: &HashMap<DescriptorPublicKey, DescriptorSecretKey>,
    network: Network,
    broadcast: impl FnOnce(S, &Transaction) -> anyhow::Result<()>,
    cmd: Commands<CS, S>,
) -> anyhow::Result<()>
where
    O::Error: std::error::Error + Send + Sync + 'static,
    C: Default + Append + DeserializeOwned + Serialize + From<KeychainChangeSet<A>>,
{
    match cmd {
        Commands::ChainSpecific(_) => unreachable!("example code should handle this!"),
        Commands::Address { addr_cmd } => {
            let graph = &mut *graph.lock().unwrap();
            let index = &mut graph.index;

            match addr_cmd {
                AddressCmd::Next | AddressCmd::New => {
                    let spk_chooser = match addr_cmd {
                        AddressCmd::Next => KeychainTxOutIndex::next_unused_spk,
                        AddressCmd::New => KeychainTxOutIndex::reveal_next_spk,
                        _ => unreachable!("only these two variants exist in match arm"),
                    };

                    let ((spk_i, spk), index_changeset) = spk_chooser(index, &Keychain::External);
                    let db = &mut *db.lock().unwrap();
                    db.stage(C::from((
                        local_chain::ChangeSet::default(),
                        indexed_tx_graph::ChangeSet::from(index_changeset),
                    )));
                    db.commit()?;
                    let addr =
                        Address::from_script(spk, network).context("failed to derive address")?;
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
        Commands::Balance => {
            let graph = &*graph.lock().unwrap();
            let chain = &*chain.lock().unwrap();
            fn print_balances<'a>(
                title_str: &'a str,
                items: impl IntoIterator<Item = (&'a str, u64)>,
            ) {
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
        Commands::TxOut { txout_cmd } => {
            let graph = &*graph.lock().unwrap();
            let chain = &*chain.lock().unwrap();
            let chain_tip = chain.get_chain_tip()?.unwrap_or_default();
            let outpoints = graph.index.outpoints().iter().cloned();

            match txout_cmd {
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
        Commands::Send {
            value,
            address,
            coin_select,
            chain_specfic,
        } => {
            let chain = &*chain.lock().unwrap();
            let address = address.require_network(network)?;
            let (transaction, change_index) = {
                let graph = &mut *graph.lock().unwrap();
                // take mutable ref to construct tx -- it is only open for a short time while building it.
                let (tx, change_info) =
                    create_tx(graph, chain, keymap, coin_select, address, value)?;

                if let Some((index_changeset, (change_keychain, index))) = change_info {
                    // We must first persist to disk the fact that we've got a new address from the
                    // change keychain so future scans will find the tx we're about to broadcast.
                    // If we're unable to persist this, then we don't want to broadcast.
                    {
                        let db = &mut *db.lock().unwrap();
                        db.stage(C::from((
                            local_chain::ChangeSet::default(),
                            indexed_tx_graph::ChangeSet::from(index_changeset),
                        )));
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

            match (broadcast)(chain_specfic, &transaction) {
                Ok(_) => {
                    println!("Broadcasted Tx : {}", transaction.txid());

                    let keychain_changeset = graph.lock().unwrap().insert_tx(transaction);

                    // We know the tx is at least unconfirmed now. Note if persisting here fails,
                    // it's not a big deal since we can always find it again form
                    // blockchain.
                    db.lock().unwrap().stage(C::from((
                        local_chain::ChangeSet::default(),
                        keychain_changeset,
                    )));
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
    }
}

#[allow(clippy::type_complexity)]
pub fn init<'m, CS: clap::Subcommand, S: clap::Args, C>(
    db_magic: &'m [u8],
    db_default_path: &str,
) -> anyhow::Result<(
    Args<CS, S>,
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
    let args = Args::<CS, S>::parse();
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
