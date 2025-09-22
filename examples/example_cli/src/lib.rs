use bdk_chain::keychain_txout::DEFAULT_LOOKAHEAD;
use serde_json::json;
use std::cmp;
use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;

use anyhow::bail;
use anyhow::Context;
use bdk_chain::bitcoin::{
    absolute, address::NetworkUnchecked, bip32, consensus, constants, hex::DisplayHex, relative,
    secp256k1::Secp256k1, transaction, Address, Amount, Network, NetworkKind, PrivateKey, Psbt,
    PublicKey, Sequence, Transaction, TxIn, TxOut,
};
use bdk_chain::miniscript::{
    descriptor::{DescriptorSecretKey, SinglePubKey},
    plan::{Assets, Plan},
    psbt::PsbtExt,
    Descriptor, DescriptorPublicKey, ForEachKey,
};
use bdk_chain::CanonicalizationParams;
use bdk_chain::ConfirmationBlockTime;
use bdk_chain::{
    indexer::keychain_txout::{self, KeychainTxOutIndex},
    local_chain::{self, LocalChain},
    tx_graph, ChainOracle, DescriptorExt, FullTxOut, IndexedTxGraph, Merge,
};
use bdk_coin_select::{
    metrics::LowestFee, Candidate, ChangePolicy, CoinSelector, DrainWeights, FeeRate, Target,
    TargetFee, TargetOutputs,
};
use bdk_file_store::Store;
use clap::{Parser, Subcommand};
use rand::prelude::*;

pub use anyhow;
pub use clap;

/// Alias for a `IndexedTxGraph` with specific `Anchor` and `Indexer`.
pub type KeychainTxGraph = IndexedTxGraph<ConfirmationBlockTime, KeychainTxOutIndex<Keychain>>;

/// ChangeSet
#[derive(Default, Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ChangeSet {
    /// Descriptor for recipient addresses.
    pub descriptor: Option<Descriptor<DescriptorPublicKey>>,
    /// Descriptor for change addresses.
    pub change_descriptor: Option<Descriptor<DescriptorPublicKey>>,
    /// Stores the network type of the transaction data.
    pub network: Option<Network>,
    /// Changes to the [`LocalChain`].
    pub local_chain: local_chain::ChangeSet,
    /// Changes to [`TxGraph`](tx_graph::TxGraph).
    pub tx_graph: tx_graph::ChangeSet<ConfirmationBlockTime>,
    /// Changes to [`KeychainTxOutIndex`].
    pub indexer: keychain_txout::ChangeSet,
}

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
pub struct Args<CS: clap::Subcommand, S: clap::Args> {
    #[clap(subcommand)]
    pub command: Commands<CS, S>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands<CS: clap::Subcommand, S: clap::Args> {
    /// Initialize a new data store.
    Init {
        /// Network
        #[clap(long, short, default_value = "signet")]
        network: Network,
        /// Descriptor
        #[clap(env = "DESCRIPTOR")]
        descriptor: String,
        /// Change descriptor
        #[clap(long, short, env = "CHANGE_DESCRIPTOR")]
        change_descriptor: Option<String>,
    },
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
    /// PSBT operations
    Psbt {
        #[clap(subcommand)]
        psbt_cmd: PsbtCmd<S>,
    },
    /// Generate new BIP86 descriptors.
    Generate {
        /// Network
        #[clap(long, short, default_value = "signet")]
        network: Network,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum AddressCmd {
    /// Get the next unused address.
    Next,
    /// Get a new address regardless of the existing unused addresses.
    New,
    /// List all addresses
    List {
        /// List change addresses
        #[clap(long)]
        change: bool,
    },
    /// Get last revealed address index for each keychain.
    Index,
}

#[derive(Subcommand, Debug, Clone)]
pub enum TxOutCmd {
    /// List transaction outputs.
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

#[derive(Subcommand, Debug, Clone)]
pub enum PsbtCmd<S: clap::Args> {
    /// Create a new PSBT.
    New {
        /// Amount to send in satoshis
        #[clap(required = true)]
        value: u64,
        /// Recipient address
        #[clap(required = true)]
        address: Address<NetworkUnchecked>,
        /// Set the feerate of the tx (sat/vbyte)
        #[clap(long, short, default_value = "1.0")]
        feerate: Option<f32>,
        /// Set max absolute timelock (from consensus value)
        #[clap(long, short)]
        after: Option<u32>,
        /// Set max relative timelock (from consensus value)
        #[clap(long, short)]
        older: Option<u32>,
        /// Coin selection algorithm
        #[clap(long, short, default_value = "bnb")]
        coin_select: CoinSelectionAlgo,
        /// Debug print the PSBT
        #[clap(long, short)]
        debug: bool,
    },
    /// Sign with a hot signer
    Sign {
        /// Private descriptor [env: DESCRIPTOR=]
        #[clap(long, short)]
        descriptor: Option<String>,
        /// PSBT
        #[clap(long, short, required = true)]
        psbt: String,
    },
    /// Extract transaction
    Extract {
        /// PSBT
        #[clap(long, short, required = true)]
        psbt: String,
        /// Whether to try broadcasting the tx
        #[clap(long, short)]
        broadcast: bool,
        #[clap(flatten)]
        chain_specific: S,
    },
}

#[derive(
    Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, serde::Deserialize, serde::Serialize,
)]
pub enum Keychain {
    External,
    Internal,
}

impl fmt::Display for Keychain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Keychain::External => write!(f, "external"),
            Keychain::Internal => write!(f, "internal"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum CoinSelectionAlgo {
    LargestFirst,
    SmallestFirst,
    OldestFirst,
    NewestFirst,
    #[default]
    BranchAndBound,
}

impl FromStr for CoinSelectionAlgo {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use CoinSelectionAlgo::*;
        Ok(match s {
            "largest-first" => LargestFirst,
            "smallest-first" => SmallestFirst,
            "oldest-first" => OldestFirst,
            "newest-first" => NewestFirst,
            "bnb" => BranchAndBound,
            unknown => bail!("unknown coin selection algorithm '{unknown}'"),
        })
    }
}

impl fmt::Display for CoinSelectionAlgo {
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

// Records changes to the internal keychain when we
// have to include a change output during tx creation.
#[derive(Debug)]
pub struct ChangeInfo {
    pub change_keychain: Keychain,
    pub indexer: keychain_txout::ChangeSet,
    pub index: u32,
}

pub fn create_tx(
    graph: &mut KeychainTxGraph,
    chain: &LocalChain,
    assets: &Assets,
    cs_algorithm: CoinSelectionAlgo,
    address: Address,
    value: u64,
    feerate: f32,
) -> anyhow::Result<(Psbt, Option<ChangeInfo>)> {
    let mut changeset = keychain_txout::ChangeSet::default();

    // get planned utxos
    let mut plan_utxos = planned_utxos(graph, chain, assets)?;

    // sort utxos if cs-algo requires it
    match cs_algorithm {
        CoinSelectionAlgo::LargestFirst => {
            plan_utxos.sort_by_key(|(_, utxo)| cmp::Reverse(utxo.txout.value))
        }
        CoinSelectionAlgo::SmallestFirst => plan_utxos.sort_by_key(|(_, utxo)| utxo.txout.value),
        CoinSelectionAlgo::OldestFirst => plan_utxos.sort_by_key(|(_, utxo)| utxo.chain_position),
        CoinSelectionAlgo::NewestFirst => {
            plan_utxos.sort_by_key(|(_, utxo)| cmp::Reverse(utxo.chain_position))
        }
        CoinSelectionAlgo::BranchAndBound => plan_utxos.shuffle(&mut thread_rng()),
    }

    // build candidate set
    let candidates: Vec<Candidate> = plan_utxos
        .iter()
        .map(|(plan, utxo)| {
            Candidate::new(
                utxo.txout.value.to_sat(),
                plan.satisfaction_weight() as u64,
                plan.witness_version().is_some(),
            )
        })
        .collect();

    // create recipient output(s)
    let mut outputs = vec![TxOut {
        value: Amount::from_sat(value),
        script_pubkey: address.script_pubkey(),
    }];

    let (change_keychain, _) = graph
        .index
        .keychains()
        .last()
        .expect("must have a keychain");

    let ((change_index, change_script), index_changeset) = graph
        .index
        .next_unused_spk(change_keychain)
        .expect("Must exist");
    changeset.merge(index_changeset);

    let mut change_output = TxOut {
        value: Amount::ZERO,
        script_pubkey: change_script,
    };

    let change_desc = graph
        .index
        .keychains()
        .find(|(k, _)| k == &change_keychain)
        .expect("must exist")
        .1;

    let min_drain_value = change_desc.dust_value().to_sat();

    let target = Target {
        outputs: TargetOutputs::fund_outputs(
            outputs
                .iter()
                .map(|output| (output.weight().to_wu(), output.value.to_sat())),
        ),
        fee: TargetFee {
            rate: FeeRate::from_sat_per_vb(feerate),
            ..Default::default()
        },
    };

    let change_policy = ChangePolicy {
        min_value: min_drain_value,
        drain_weights: DrainWeights::TR_KEYSPEND,
    };

    // run coin selection
    let mut selector = CoinSelector::new(&candidates);
    match cs_algorithm {
        CoinSelectionAlgo::BranchAndBound => {
            let metric = LowestFee {
                target,
                long_term_feerate: FeeRate::from_sat_per_vb(10.0),
                change_policy,
            };
            match selector.run_bnb(metric, 10_000) {
                Ok(_) => {}
                Err(_) => selector
                    .select_until_target_met(target)
                    .context("selecting coins")?,
            }
        }
        _ => selector
            .select_until_target_met(target)
            .context("selecting coins")?,
    }

    // get the selected plan utxos
    let selected: Vec<_> = selector.apply_selection(&plan_utxos).collect();

    // if the selection tells us to use change and the change value is sufficient, we add it as an
    // output
    let mut change_info = Option::<ChangeInfo>::None;
    let drain = selector.drain(target, change_policy);
    if drain.value > min_drain_value {
        change_output.value = Amount::from_sat(drain.value);
        outputs.push(change_output);
        change_info = Some(ChangeInfo {
            change_keychain,
            indexer: changeset,
            index: change_index,
        });
        outputs.shuffle(&mut thread_rng());
    }

    let unsigned_tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: assets
            .absolute_timelock
            .unwrap_or(absolute::LockTime::from_height(
                chain.get_chain_tip()?.height,
            )?),
        input: selected
            .iter()
            .map(|(plan, utxo)| TxIn {
                previous_output: utxo.outpoint,
                sequence: plan
                    .relative_timelock
                    .map_or(Sequence::ENABLE_RBF_NO_LOCKTIME, Sequence::from),
                ..Default::default()
            })
            .collect(),
        output: outputs,
    };

    // update psbt with plan
    let mut psbt = Psbt::from_unsigned_tx(unsigned_tx)?;
    for (i, (plan, utxo)) in selected.iter().enumerate() {
        let psbt_input = &mut psbt.inputs[i];
        plan.update_psbt_input(psbt_input);
        psbt_input.witness_utxo = Some(utxo.txout.clone());
    }

    Ok((psbt, change_info))
}

// Alias the elements of `planned_utxos`
pub type PlanUtxo = (Plan, FullTxOut<ConfirmationBlockTime>);

pub fn planned_utxos(
    graph: &KeychainTxGraph,
    chain: &LocalChain,
    assets: &Assets,
) -> Result<Vec<PlanUtxo>, Infallible> {
    let chain_tip = chain.tip().block_id();
    let outpoints = graph.index.outpoints();
    let task = graph
        .graph()
        .canonicalization_task(CanonicalizationParams::default());
    chain
        .canonicalize(task, Some(chain_tip))
        .filter_unspent_outpoints(outpoints.iter().cloned())
        .filter_map(|((k, i), full_txo)| -> Option<Result<PlanUtxo, _>> {
            let desc = graph
                .index
                .keychains()
                .find(|(keychain, _)| *keychain == k)
                .expect("keychain must exist")
                .1
                .at_derivation_index(i)
                .expect("i can't be hardened");

            let plan = desc.plan(assets).ok()?;

            Some(Ok((plan, full_txo)))
        })
        .collect()
}

pub fn handle_commands<CS: clap::Subcommand, S: clap::Args>(
    graph: &Mutex<KeychainTxGraph>,
    chain: &Mutex<LocalChain>,
    db: &Mutex<Store<ChangeSet>>,
    network: Network,
    broadcast_fn: impl FnOnce(S, &Transaction) -> anyhow::Result<()>,
    cmd: Commands<CS, S>,
) -> anyhow::Result<()> {
    match cmd {
        Commands::Init { .. } => unreachable!("handled by init command"),
        Commands::Generate { .. } => unreachable!("handled by generate command"),
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

                    let ((spk_i, spk), index_changeset) =
                        spk_chooser(index, Keychain::External).expect("Must exist");
                    let db = &mut *db.lock().unwrap();
                    db.append(&ChangeSet {
                        indexer: index_changeset,
                        ..Default::default()
                    })?;
                    let addr = Address::from_script(spk.as_script(), network)?;
                    println!("[address @ {spk_i}] {addr}");
                    Ok(())
                }
                AddressCmd::Index => {
                    for (keychain, derivation_index) in index.last_revealed_indices() {
                        println!("{keychain:?}: {derivation_index}");
                    }
                    Ok(())
                }
                AddressCmd::List { change } => {
                    let target_keychain = match change {
                        true => Keychain::Internal,
                        false => Keychain::External,
                    };
                    for (spk_i, spk) in index.revealed_keychain_spks(target_keychain) {
                        let address = Address::from_script(spk.as_script(), network)
                            .expect("should always be able to derive address");
                        println!(
                            "{:?} {} used:{}",
                            spk_i,
                            address,
                            index.is_used(target_keychain, spk_i)
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
                items: impl IntoIterator<Item = (&'a str, Amount)>,
            ) {
                println!("{title_str}:");
                for (name, amount) in items.into_iter() {
                    println!("    {:<10} {:>12} sats", name, amount.to_sat())
                }
            }

            let task = graph
                .graph()
                .canonicalization_task(CanonicalizationParams::default());
            let balance = chain
                .canonicalize(task, Some(chain.tip().block_id()))
                .balance(
                    graph.index.outpoints().iter().cloned(),
                    |(k, _), _| k == &Keychain::Internal,
                    1,
                );

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
            let chain_tip = chain.get_chain_tip()?;
            let outpoints = graph.index.outpoints();

            match txout_cmd {
                TxOutCmd::List {
                    spent,
                    unspent,
                    confirmed,
                    unconfirmed,
                } => {
                    let task = graph
                        .graph()
                        .canonicalization_task(CanonicalizationParams::default());
                    let txouts = chain
                        .canonicalize(task, Some(chain_tip))
                        .filter_outpoints(outpoints.iter().cloned())
                        .filter(|(_, full_txo)| match (spent, unspent) {
                            (true, false) => full_txo.spent_by.is_some(),
                            (false, true) => full_txo.spent_by.is_none(),
                            _ => true,
                        })
                        .filter(|(_, full_txo)| match (confirmed, unconfirmed) {
                            (true, false) => full_txo.chain_position.is_confirmed(),
                            (false, true) => !full_txo.chain_position.is_confirmed(),
                            _ => true,
                        })
                        .collect::<Vec<_>>();

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
        Commands::Psbt { psbt_cmd } => match psbt_cmd {
            PsbtCmd::New {
                value,
                address,
                feerate,
                after,
                older,
                coin_select,
                debug,
            } => {
                let address = address.require_network(network)?;

                let (psbt, change_info) = {
                    let mut graph = graph.lock().unwrap();
                    let chain = chain.lock().unwrap();

                    // collect assets we can sign for
                    let mut pks = vec![];
                    for (_, desc) in graph.index.keychains() {
                        desc.for_each_key(|k| {
                            pks.push(k.clone());
                            true
                        });
                    }
                    let mut assets = Assets::new().add(pks);
                    if let Some(n) = after {
                        assets = assets.after(absolute::LockTime::from_consensus(n));
                    }
                    if let Some(n) = older {
                        assets = assets.older(relative::LockTime::from_consensus(n)?);
                    }

                    create_tx(
                        &mut graph,
                        &chain,
                        &assets,
                        coin_select,
                        address,
                        value,
                        feerate.expect("must have feerate"),
                    )?
                };

                if let Some(ChangeInfo {
                    change_keychain,
                    indexer,
                    index,
                }) = change_info
                {
                    // We must first persist to disk the fact that we've got a new address from the
                    // change keychain so future scans will find the tx we're about to broadcast.
                    // If we're unable to persist this, then we don't want to broadcast.
                    {
                        let db = &mut *db.lock().unwrap();
                        db.append(&ChangeSet {
                            indexer,
                            ..Default::default()
                        })?;
                    }

                    // We don't want other callers/threads to use this address while we're using it
                    // but we also don't want to scan the tx we just created because it's not
                    // technically in the blockchain yet.
                    graph
                        .lock()
                        .unwrap()
                        .index
                        .mark_used(change_keychain, index);
                }

                if debug {
                    dbg!(psbt);
                } else {
                    // print base64 encoded psbt
                    let fee = psbt.fee()?.to_sat();
                    let mut obj = serde_json::Map::new();
                    obj.insert("psbt".to_string(), json!(psbt.to_string()));
                    obj.insert("fee".to_string(), json!(fee));
                    println!("{}", serde_json::to_string_pretty(&obj)?);
                };

                Ok(())
            }
            PsbtCmd::Sign { psbt, descriptor } => {
                let mut psbt = Psbt::from_str(&psbt)?;

                let desc_str = match descriptor {
                    Some(s) => s,
                    None => env::var("DESCRIPTOR").context("unable to sign")?,
                };

                let secp = Secp256k1::new();
                let (_, keymap) = Descriptor::parse_descriptor(&secp, &desc_str)?;
                if keymap.is_empty() {
                    bail!("unable to sign")
                }

                // note: we're only looking at the first entry in the keymap
                // the idea is to find something that impls `GetKey`
                let sign_res = match keymap.iter().next().expect("not empty") {
                    (DescriptorPublicKey::Single(single_pub), DescriptorSecretKey::Single(prv)) => {
                        let pk = match single_pub.key {
                            SinglePubKey::FullKey(pk) => pk,
                            SinglePubKey::XOnly(_) => unimplemented!("single xonly pubkey"),
                        };
                        let keys: HashMap<PublicKey, PrivateKey> = [(pk, prv.key)].into();
                        psbt.sign(&keys, &secp)
                    }
                    (_, DescriptorSecretKey::XPrv(k)) => psbt.sign(&k.xkey, &secp),
                    _ => unimplemented!("multi xkey signer"),
                };

                let _ =
                    sign_res.map_err(|errors| anyhow::anyhow!("failed to sign PSBT {errors:?}"))?;

                let mut obj = serde_json::Map::new();
                obj.insert("psbt".to_string(), json!(psbt.to_string()));
                println!("{}", serde_json::to_string_pretty(&obj)?);

                Ok(())
            }
            PsbtCmd::Extract {
                broadcast,
                chain_specific,
                psbt,
            } => {
                let mut psbt = Psbt::from_str(&psbt)?;
                psbt.finalize_mut(&Secp256k1::new())
                    .map_err(|errors| anyhow::anyhow!("failed to finalize PSBT {errors:?}"))?;

                let tx = psbt.extract_tx()?;

                if broadcast {
                    let mut graph = graph.lock().unwrap();

                    match broadcast_fn(chain_specific, &tx) {
                        Ok(_) => {
                            println!("Broadcasted Tx: {}", tx.compute_txid());

                            let changeset = graph.insert_tx(tx);

                            // We know the tx is at least unconfirmed now. Note if persisting here
                            // fails, it's not a big deal since we can
                            // always find it again from the blockchain.
                            db.lock().unwrap().append(&ChangeSet {
                                tx_graph: changeset.tx_graph,
                                indexer: changeset.indexer,
                                ..Default::default()
                            })?;
                        }
                        Err(e) => {
                            // We failed to broadcast, so allow our change address to be used in the
                            // future
                            let (change_keychain, _) = graph
                                .index
                                .keychains()
                                .last()
                                .expect("must have a keychain");
                            let change_index = tx.output.iter().find_map(|txout| {
                                let spk = txout.script_pubkey.as_script();
                                match graph.index.index_of_spk(spk) {
                                    Some(&(keychain, index)) if keychain == change_keychain => {
                                        Some((keychain, index))
                                    }
                                    _ => None,
                                }
                            });
                            if let Some((keychain, index)) = change_index {
                                graph.index.unmark_used(keychain, index);
                            }
                            bail!(e);
                        }
                    }
                } else {
                    // encode raw tx hex
                    let hex = consensus::serialize(&tx).to_lower_hex_string();
                    let mut obj = serde_json::Map::new();
                    obj.insert("tx".to_string(), json!(hex));
                    println!("{}", serde_json::to_string_pretty(&obj)?);
                }

                Ok(())
            }
        },
    }
}

/// The initial state returned by [`init_or_load`].
pub struct Init<CS: clap::Subcommand, S: clap::Args> {
    /// CLI args
    pub args: Args<CS, S>,
    /// Indexed graph
    pub graph: Mutex<KeychainTxGraph>,
    /// Local chain
    pub chain: Mutex<LocalChain>,
    /// Database
    pub db: Mutex<Store<ChangeSet>>,
    /// Network
    pub network: Network,
}

/// Loads from persistence or creates new
pub fn init_or_load<CS: clap::Subcommand, S: clap::Args>(
    db_magic: &[u8],
    db_path: &str,
) -> anyhow::Result<Option<Init<CS, S>>> {
    let args = Args::<CS, S>::parse();

    match args.command {
        // initialize new db
        Commands::Init { .. } => initialize::<CS, S>(args, db_magic, db_path).map(|_| None),
        // generate keys
        Commands::Generate { network } => generate_bip86_helper(network).map(|_| None),
        // try load
        _ => {
            let (mut db, changeset) =
                Store::<ChangeSet>::load(db_magic, db_path).context("could not open file store")?;

            let changeset = changeset.expect("should not be empty");
            let network = changeset.network.expect("changeset network");

            let chain = Mutex::new({
                let (mut chain, _) =
                    LocalChain::from_genesis(constants::genesis_block(network).block_hash());
                chain.apply_changeset(&changeset.local_chain)?;
                chain
            });

            let (graph, changeset) = IndexedTxGraph::from_changeset(
                (changeset.tx_graph, changeset.indexer).into(),
                |c| -> anyhow::Result<_> {
                    let mut indexer =
                        KeychainTxOutIndex::from_changeset(DEFAULT_LOOKAHEAD, true, c);
                    if let Some(desc) = changeset.descriptor {
                        indexer.insert_descriptor(Keychain::External, desc)?;
                    }
                    if let Some(change_desc) = changeset.change_descriptor {
                        indexer.insert_descriptor(Keychain::Internal, change_desc)?;
                    }
                    Ok(indexer)
                },
            )?;
            db.append(&ChangeSet {
                indexer: changeset.indexer,
                tx_graph: changeset.tx_graph,
                ..Default::default()
            })?;

            let graph = Mutex::new(graph);
            let db = Mutex::new(db);

            Ok(Some(Init {
                args,
                graph,
                chain,
                db,
                network,
            }))
        }
    }
}

/// Initialize db backend.
fn initialize<CS, S>(args: Args<CS, S>, db_magic: &[u8], db_path: &str) -> anyhow::Result<()>
where
    CS: clap::Subcommand,
    S: clap::Args,
{
    if let Commands::Init {
        network,
        descriptor,
        change_descriptor,
    } = args.command
    {
        let mut changeset = ChangeSet::default();

        // parse descriptors
        let secp = Secp256k1::new();
        let mut index = KeychainTxOutIndex::default();
        let (descriptor, _) =
            Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &descriptor)?;
        let _ = index.insert_descriptor(Keychain::External, descriptor.clone())?;
        changeset.descriptor = Some(descriptor);

        if let Some(desc) = change_descriptor {
            let (change_descriptor, _) =
                Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, &desc)?;
            let _ = index.insert_descriptor(Keychain::Internal, change_descriptor.clone())?;
            changeset.change_descriptor = Some(change_descriptor);
        }

        // create new
        let (_, chain_changeset) =
            LocalChain::from_genesis(constants::genesis_block(network).block_hash());
        changeset.network = Some(network);
        changeset.local_chain = chain_changeset;
        let mut db = Store::<ChangeSet>::create(db_magic, db_path)?;
        db.append(&changeset)?;
        println!("New database {db_path}");
    }

    Ok(())
}

/// Generate BIP86 descriptors.
fn generate_bip86_helper(network: impl Into<NetworkKind>) -> anyhow::Result<()> {
    let secp = Secp256k1::new();
    let mut seed = [0x00; 32];
    thread_rng().fill_bytes(&mut seed);

    let m = bip32::Xpriv::new_master(network, &seed)?;
    let fp = m.fingerprint(&secp);
    let path = if m.network.is_mainnet() {
        "86h/0h/0h"
    } else {
        "86h/1h/0h"
    };

    let descriptors: Vec<String> = [0, 1]
        .iter()
        .map(|i| format!("tr([{fp}]{m}/{path}/{i}/*)"))
        .collect();
    let external_desc = &descriptors[0];
    let internal_desc = &descriptors[1];
    let (descriptor, keymap) =
        <Descriptor<DescriptorPublicKey>>::parse_descriptor(&secp, external_desc)?;
    let (internal_descriptor, internal_keymap) =
        <Descriptor<DescriptorPublicKey>>::parse_descriptor(&secp, internal_desc)?;
    println!("Public");
    println!("{descriptor}");
    println!("{internal_descriptor}");
    println!("\nPrivate");
    println!("{}", descriptor.to_string_with_secret(&keymap));
    println!(
        "{}",
        internal_descriptor.to_string_with_secret(&internal_keymap)
    );

    Ok(())
}

impl Merge for ChangeSet {
    fn merge(&mut self, other: Self) {
        if other.descriptor.is_some() {
            self.descriptor = other.descriptor;
        }
        if other.change_descriptor.is_some() {
            self.change_descriptor = other.change_descriptor;
        }
        if other.network.is_some() {
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
