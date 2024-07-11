use std::{
    io::{self, Write},
    sync::Mutex,
};

use bdk_chain::{
    bitcoin::{constants::genesis_block, Address, Network, Txid},
    collections::BTreeSet,
    indexer::keychain_txout,
    local_chain::{self, LocalChain},
    spk_client::{FullScanRequest, SyncRequest},
    tx_graph, ConfirmationBlockTime, Merge, TxGraph,
};
use bdk_electrum::{
    electrum_client::{self, Client, ElectrumApi},
    BdkElectrumClient,
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

#[derive(Parser, Debug, Clone, PartialEq)]
pub struct ScanOptions {
    /// Set batch size for each script_history call to electrum client.
    #[clap(long, default_value = "25")]
    pub batch_size: usize,
}

type ChangeSet = (
    local_chain::ChangeSet,
    tx_graph::ChangeSet<ConfirmationBlockTime, keychain_txout::ChangeSet<Keychain>>,
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
        let mut graph = TxGraph::new(index);
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

    let client = BdkElectrumClient::new(electrum_cmd.electrum_args().client(args.network)?);

    // Tell the electrum client about the txs we've already got locally so it doesn't re-download them
    client.populate_tx_cache(&*graph.lock().unwrap());

    let (local_chain_changeset, tx_graph_changeset) = match electrum_cmd.clone() {
        ElectrumCommands::Scan {
            stop_gap,
            scan_options,
            ..
        } => {
            let request = {
                let graph = &*graph.lock().unwrap();
                let chain = &*chain.lock().unwrap();

                FullScanRequest::from_chain_tip(chain.tip())
                    .set_spks_for_keychain(
                        Keychain::External,
                        graph
                            .indexer
                            .unbounded_spk_iter(&Keychain::External)
                            .into_iter()
                            .flatten(),
                    )
                    .set_spks_for_keychain(
                        Keychain::Internal,
                        graph
                            .indexer
                            .unbounded_spk_iter(&Keychain::Internal)
                            .into_iter()
                            .flatten(),
                    )
                    .inspect_spks_for_all_keychains({
                        let mut once = BTreeSet::new();
                        move |k, spk_i, _| {
                            if once.insert(k) {
                                eprint!("\nScanning {}: {} ", k, spk_i);
                            } else {
                                eprint!("{} ", spk_i);
                            }
                            io::stdout().flush().expect("must flush");
                        }
                    })
            };

            let mut update = client
                .full_scan::<_>(request, stop_gap, scan_options.batch_size, false)
                .context("scanning the blockchain")?;

            // We want to keep track of the latest time a transaction was seen unconfirmed.
            let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
            let _ = update.graph_update.update_last_seen_unconfirmed(now);

            let mut graph = graph.lock().expect("mutex must not be poisoned");
            let mut chain = chain.lock().expect("mutex must not be poisoned");

            // Because we did a stop gap based scan we are likely to have some updates to our
            // deriviation indices. Usually before a scan you are on a fresh wallet with no
            // addresses derived so we need to derive up to last active addresses the scan found
            // before adding the transactions.
            (chain.apply_update(update.chain_update)?, {
                let index_changeset = graph
                    .indexer
                    .reveal_to_target_multi(&update.last_active_indices);
                let mut tx_graph_changeset = graph.apply_update(update.graph_update);
                tx_graph_changeset.merge(index_changeset.into());
                tx_graph_changeset
            })
        }
        ElectrumCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
            ..
        } => {
            let local_tip = chain.lock().expect("mutex must not be poisoned").tip();
            // Spks, outpoints and txids we want updates on will be accumulated here.
            let mut request = SyncRequest::from_chain_tip(local_tip.clone());

            {
                // Get a short lock on the tracker to get the spks we're interested in
                let graph = graph.lock().unwrap();
                let chain = chain.lock().unwrap();

                if !(all_spks || unused_spks || utxos || unconfirmed) {
                    unused_spks = true;
                    unconfirmed = true;
                    utxos = true;
                } else if all_spks {
                    unused_spks = false;
                }

                let chain_tip = chain.tip();

                if all_spks {
                    let all_spks = graph
                        .indexer
                        .revealed_spks(..)
                        .map(|(index, spk)| (index, spk.to_owned()))
                        .collect::<Vec<_>>();
                    request = request.chain_spks(all_spks.into_iter().map(|((k, spk_i), spk)| {
                        eprint!("Scanning {}: {}", k, spk_i);
                        spk
                    }));
                }
                if unused_spks {
                    let unused_spks = graph
                        .indexer
                        .unused_spks()
                        .map(|(index, spk)| (index, spk.to_owned()))
                        .collect::<Vec<_>>();
                    request = request.chain_spks(unused_spks.into_iter().map(
                        move |((k, spk_i), spk)| {
                            eprint!(
                                "Checking if address {} {}:{} has been used",
                                Address::from_script(&spk, args.network).unwrap(),
                                k,
                                spk_i,
                            );
                            spk
                        },
                    ));
                }

                if utxos {
                    let init_outpoints = graph.indexer.outpoints();

                    let utxos = graph
                        .filter_chain_unspents(
                            &*chain,
                            chain_tip.block_id(),
                            init_outpoints.iter().cloned(),
                        )
                        .map(|(_, utxo)| utxo)
                        .collect::<Vec<_>>();
                    request = request.chain_outpoints(utxos.into_iter().map(|utxo| {
                        eprint!(
                            "Checking if outpoint {} (value: {}) has been spent",
                            utxo.outpoint, utxo.txout.value
                        );
                        utxo.outpoint
                    }));
                };

                if unconfirmed {
                    let unconfirmed_txids = graph
                        .list_canonical_txs(&*chain, chain_tip.block_id())
                        .filter(|canonical_tx| !canonical_tx.chain_position.is_confirmed())
                        .map(|canonical_tx| canonical_tx.tx_node.txid)
                        .collect::<Vec<Txid>>();

                    request = request.chain_txids(
                        unconfirmed_txids
                            .into_iter()
                            .inspect(|txid| eprint!("Checking if {} is confirmed yet", txid)),
                    );
                }

                let total_spks = request.spks.len();
                let total_txids = request.txids.len();
                let total_ops = request.outpoints.len();
                request = request
                    .inspect_spks({
                        let mut visited = 0;
                        move |_| {
                            visited += 1;
                            eprintln!(" [ {:>6.2}% ]", (visited * 100) as f32 / total_spks as f32)
                        }
                    })
                    .inspect_txids({
                        let mut visited = 0;
                        move |_| {
                            visited += 1;
                            eprintln!(" [ {:>6.2}% ]", (visited * 100) as f32 / total_txids as f32)
                        }
                    })
                    .inspect_outpoints({
                        let mut visited = 0;
                        move |_| {
                            visited += 1;
                            eprintln!(" [ {:>6.2}% ]", (visited * 100) as f32 / total_ops as f32)
                        }
                    });
            }

            let mut update = client
                .sync(request, scan_options.batch_size, false)
                .context("scanning the blockchain")?;

            let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
            let _ = update.graph_update.update_last_seen_unconfirmed(now);

            (
                chain.lock().unwrap().apply_update(update.chain_update)?,
                graph.lock().unwrap().apply_update(update.graph_update),
            )
        }
    };

    // We persist the changes
    let mut db = db.lock().unwrap();
    db.append_changeset(&(local_chain_changeset, tx_graph_changeset))?;

    Ok(())
}
