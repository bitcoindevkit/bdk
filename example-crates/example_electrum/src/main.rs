#![allow(clippy::needless_collect)]
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Mutex,
};

use bdk_chain::{
    bitcoin::{constants::genesis_block, Address, Network, OutPoint, Txid},
    indexed_tx_graph::{self, IndexedTxGraph},
    keychain,
    local_chain::{self, LocalChain},
    Append, ConfirmationHeightAnchor,
};
use bdk_electrum::{
    electrum_client::{self, Client, ElectrumApi},
    ElectrumExt, ElectrumUpdate,
};
use example_cli::{
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
    Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_electrum";
const DB_PATH: &str = ".bdk_example_electrum.db";

#[derive(Subcommand, Debug, Clone)]
enum ElectrumCommands {
    /// Scans the addresses in the wallet using the electrum API.
    Scan {
        /// When a gap this large has been found for a keychain, it will stop.
        #[clap(long, default_value = "5")]
        stop_gap: usize,
        #[clap(flatten)]
        scan_options: ScanOptions,
        #[clap(flatten)]
        electrum_args: ElectrumArgs,
    },
    /// Scans particular addresses using the electrum API.
    Sync {
        /// Scan all the unused addresses.
        #[clap(long)]
        unused_spks: bool,
        /// Scan every address that you have derived.
        #[clap(long)]
        all_spks: bool,
        /// Scan unspent outpoints for spends or changes to confirmation status of residing tx.
        #[clap(long)]
        utxos: bool,
        /// Scan unconfirmed transactions for updates.
        #[clap(long)]
        unconfirmed: bool,
        #[clap(flatten)]
        scan_options: ScanOptions,
        #[clap(flatten)]
        electrum_args: ElectrumArgs,
    },
}

impl ElectrumCommands {
    fn electrum_args(&self) -> ElectrumArgs {
        match self {
            ElectrumCommands::Scan { electrum_args, .. } => electrum_args.clone(),
            ElectrumCommands::Sync { electrum_args, .. } => electrum_args.clone(),
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct ElectrumArgs {
    /// The electrum url to use to connect to. If not provided it will use a default electrum server
    /// for your chosen network.
    electrum_url: Option<String>,
}

impl ElectrumArgs {
    pub fn client(&self, network: Network) -> anyhow::Result<Client> {
        let electrum_url = self.electrum_url.as_deref().unwrap_or(match network {
            Network::Bitcoin => "ssl://electrum.blockstream.info:50002",
            Network::Testnet => "ssl://electrum.blockstream.info:60002",
            Network::Regtest => "tcp://localhost:60401",
            Network::Signet => "tcp://signet-electrumx.wakiyamap.dev:50001",
            _ => panic!("Unknown network"),
        });
        let config = electrum_client::Config::builder()
            .validate_domain(matches!(network, Network::Bitcoin))
            .build();

        Ok(electrum_client::Client::from_config(electrum_url, config)?)
    }
}

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// Set batch size for each script_history call to electrum client.
    #[clap(long, default_value = "25")]
    pub batch_size: usize,
}

type ChangeSet = (
    local_chain::ChangeSet,
    indexed_tx_graph::ChangeSet<ConfirmationHeightAnchor, keychain::ChangeSet<Keychain>>,
);

fn main() -> anyhow::Result<()> {
    let example_cli::Init {
        args,
        keymap,
        index,
        db,
        init_changeset,
    } = example_cli::init::<ElectrumCommands, ElectrumArgs, ChangeSet>(DB_MAGIC, DB_PATH)?;

    let (disk_local_chain, disk_tx_graph) = init_changeset;

    let graph = Mutex::new({
        let mut graph = IndexedTxGraph::new(index);
        graph.apply_changeset(disk_tx_graph);
        graph
    });

    let chain = Mutex::new({
        let genesis_hash = genesis_block(args.network).block_hash();
        let (mut chain, _) = LocalChain::from_genesis_hash(genesis_hash);
        chain.apply_changeset(&disk_local_chain)?;
        chain
    });

    let electrum_cmd = match &args.command {
        example_cli::Commands::ChainSpecific(electrum_cmd) => electrum_cmd,
        general_cmd => {
            return example_cli::handle_commands(
                &graph,
                &db,
                &chain,
                &keymap,
                args.network,
                |electrum_args, tx| {
                    let client = electrum_args.client(args.network)?;
                    client.transaction_broadcast(tx)?;
                    Ok(())
                },
                general_cmd.clone(),
            );
        }
    };

    let client = electrum_cmd.electrum_args().client(args.network)?;

    let response = match electrum_cmd.clone() {
        ElectrumCommands::Scan {
            stop_gap,
            scan_options,
            ..
        } => {
            let (keychain_spks, tip) = {
                let graph = &*graph.lock().unwrap();
                let chain = &*chain.lock().unwrap();

                let keychain_spks = graph
                    .index
                    .all_unbounded_spk_iters()
                    .into_iter()
                    .map(|(keychain, iter)| {
                        let mut first = true;
                        let spk_iter = iter.inspect(move |(i, _)| {
                            if first {
                                eprint!("\nscanning {}: ", keychain);
                                first = false;
                            }

                            eprint!("{} ", i);
                            let _ = io::stdout().flush();
                        });
                        (keychain, spk_iter)
                    })
                    .collect::<BTreeMap<_, _>>();

                let tip = chain.tip();
                (keychain_spks, tip)
            };

            client
                .full_scan(tip, keychain_spks, stop_gap, scan_options.batch_size)
                .context("scanning the blockchain")?
        }
        ElectrumCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
            ..
        } => {
            // Get a short lock on the tracker to get the spks we're interested in
            let graph = graph.lock().unwrap();
            let chain = chain.lock().unwrap();
            let chain_tip = chain.tip().block_id();

            if !(all_spks || unused_spks || utxos || unconfirmed) {
                unused_spks = true;
                unconfirmed = true;
                utxos = true;
            } else if all_spks {
                unused_spks = false;
            }

            let mut spks: Box<dyn Iterator<Item = bdk_chain::bitcoin::ScriptBuf>> =
                Box::new(core::iter::empty());
            if all_spks {
                let all_spks = graph
                    .index
                    .revealed_spks()
                    .map(|(k, i, spk)| (k, i, spk.to_owned()))
                    .collect::<Vec<_>>();
                spks = Box::new(spks.chain(all_spks.into_iter().map(|(k, i, spk)| {
                    eprintln!("scanning {}:{}", k, i);
                    spk
                })));
            }
            if unused_spks {
                let unused_spks = graph
                    .index
                    .unused_spks()
                    .map(|(k, i, spk)| (k, i, spk.to_owned()))
                    .collect::<Vec<_>>();
                spks = Box::new(spks.chain(unused_spks.into_iter().map(|(k, i, spk)| {
                    eprintln!(
                        "Checking if address {} {}:{} has been used",
                        Address::from_script(&spk, args.network).unwrap(),
                        k,
                        i,
                    );
                    spk
                })));
            }

            let mut outpoints: Box<dyn Iterator<Item = OutPoint>> = Box::new(core::iter::empty());

            if utxos {
                let init_outpoints = graph.index.outpoints().iter().cloned();

                let utxos = graph
                    .graph()
                    .filter_chain_unspents(&*chain, chain_tip, init_outpoints)
                    .map(|(_, utxo)| utxo)
                    .collect::<Vec<_>>();

                outpoints = Box::new(
                    utxos
                        .into_iter()
                        .inspect(|utxo| {
                            eprintln!(
                                "Checking if outpoint {} (value: {}) has been spent",
                                utxo.outpoint, utxo.txout.value
                            );
                        })
                        .map(|utxo| utxo.outpoint),
                );
            };

            let mut txids: Box<dyn Iterator<Item = Txid>> = Box::new(core::iter::empty());

            if unconfirmed {
                let unconfirmed_txids = graph
                    .graph()
                    .list_chain_txs(&*chain, chain_tip)
                    .filter(|canonical_tx| !canonical_tx.chain_position.is_confirmed())
                    .map(|canonical_tx| canonical_tx.tx_node.txid)
                    .collect::<Vec<Txid>>();

                txids = Box::new(unconfirmed_txids.into_iter().inspect(|txid| {
                    eprintln!("Checking if {} is confirmed yet", txid);
                }));
            }

            let tip = chain.tip();

            // drop lock on graph and chain
            drop((graph, chain));

            let electrum_update = client
                .sync(tip, spks, txids, outpoints, scan_options.batch_size)
                .context("scanning the blockchain")?;
            (electrum_update, BTreeMap::new())
        }
    };

    let (
        ElectrumUpdate {
            chain_update,
            relevant_txids,
        },
        keychain_update,
    ) = response;

    let missing_txids = {
        let graph = &*graph.lock().unwrap();
        relevant_txids.missing_full_txs(graph.graph())
    };

    let now = std::time::UNIX_EPOCH
        .elapsed()
        .expect("must get time")
        .as_secs();

    let graph_update = relevant_txids.into_tx_graph(&client, Some(now), missing_txids)?;

    let db_changeset = {
        let mut chain = chain.lock().unwrap();
        let mut graph = graph.lock().unwrap();

        let chain = chain.apply_update(chain_update)?;

        let indexed_tx_graph = {
            let mut changeset =
                indexed_tx_graph::ChangeSet::<ConfirmationHeightAnchor, _>::default();
            let (_, indexer) = graph.index.reveal_to_target_multi(&keychain_update);
            changeset.append(indexed_tx_graph::ChangeSet {
                indexer,
                ..Default::default()
            });
            changeset.append(graph.apply_update(graph_update));
            changeset
        };

        (chain, indexed_tx_graph)
    };

    let mut db = db.lock().unwrap();
    db.stage(db_changeset);
    db.commit()?;
    Ok(())
}
