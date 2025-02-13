use std::io::{self, Write};

use bdk_chain::{
    bitcoin::Network,
    collections::BTreeSet,
    indexed_tx_graph,
    spk_client::{FullScanRequest, SyncRequest},
    ConfirmationBlockTime, Merge,
};
use bdk_electrum::{
    electrum_client::{self, Client, ElectrumApi},
    BdkElectrumClient,
};
use example_cli::{
    self,
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
    ChangeSet, Keychain,
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

fn main() -> anyhow::Result<()> {
    let example_cli::Init {
        args,
        graph,
        chain,
        db,
        network,
    } = match example_cli::init_or_load::<ElectrumCommands, ElectrumArgs>(DB_MAGIC, DB_PATH)? {
        Some(init) => init,
        None => return Ok(()),
    };

    let electrum_cmd = match &args.command {
        example_cli::Commands::ChainSpecific(electrum_cmd) => electrum_cmd,
        general_cmd => {
            return example_cli::handle_commands(
                &graph,
                &chain,
                &db,
                network,
                |electrum_args, tx| {
                    let client = electrum_args.client(network)?;
                    client.transaction_broadcast(tx)?;
                    Ok(())
                },
                general_cmd.clone(),
            );
        }
    };

    let client = BdkElectrumClient::new(electrum_cmd.electrum_args().client(network)?);

    // Tell the electrum client about the txs we've already got locally so it doesn't re-download them
    client.populate_tx_cache(
        graph
            .lock()
            .unwrap()
            .graph()
            .full_txs()
            .map(|tx_node| tx_node.tx),
    );

    let (chain_update, tx_update, keychain_update) = match electrum_cmd.clone() {
        ElectrumCommands::Scan {
            stop_gap,
            scan_options,
            ..
        } => {
            let request = {
                let graph = &*graph.lock().unwrap();
                let chain = &*chain.lock().unwrap();

                FullScanRequest::builder_now()
                    .chain_tip(chain.tip())
                    .spks_for_keychain(
                        Keychain::External,
                        graph
                            .index
                            .unbounded_spk_iter(Keychain::External)
                            .into_iter()
                            .flatten(),
                    )
                    .spks_for_keychain(
                        Keychain::Internal,
                        graph
                            .index
                            .unbounded_spk_iter(Keychain::Internal)
                            .into_iter()
                            .flatten(),
                    )
                    .inspect({
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

            let res = client
                .full_scan::<_>(request, stop_gap, scan_options.batch_size, false)
                .context("scanning the blockchain")?;
            (
                res.chain_update,
                res.tx_update,
                Some(res.last_active_indices),
            )
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

            if !(all_spks || unused_spks || utxos || unconfirmed) {
                unused_spks = true;
                unconfirmed = true;
                utxos = true;
            } else if all_spks {
                unused_spks = false;
            }

            let chain_tip = chain.tip();
            let mut request = SyncRequest::builder_now()
                .chain_tip(chain_tip.clone())
                .inspect(|item, progress| {
                    let pc = (100 * progress.consumed()) as f32 / progress.total() as f32;
                    eprintln!("[ SCANNING {:03.0}% ] {}", pc, item);
                });

            if all_spks {
                request = request.spks_with_indexes(graph.index.revealed_spks(..));
            }
            if unused_spks {
                request = request.spks_with_indexes(graph.index.unused_spks());
            }
            if utxos {
                let init_outpoints = graph.index.outpoints();
                request = request.outpoints(
                    graph
                        .graph()
                        .filter_chain_unspents(
                            &*chain,
                            chain_tip.block_id(),
                            init_outpoints.iter().cloned(),
                        )
                        .map(|(_, utxo)| utxo.outpoint),
                );
            };
            if unconfirmed {
                request = request.txids(
                    graph
                        .graph()
                        .list_canonical_txs(&*chain, chain_tip.block_id())
                        .filter(|canonical_tx| !canonical_tx.chain_position.is_confirmed())
                        .map(|canonical_tx| canonical_tx.tx_node.txid),
                );
            }

            let res = client
                .sync(request, scan_options.batch_size, false)
                .context("scanning the blockchain")?;

            // drop lock on graph and chain
            drop((graph, chain));

            (res.chain_update, res.tx_update, None)
        }
    };

    let db_changeset = {
        let mut chain = chain.lock().unwrap();
        let mut graph = graph.lock().unwrap();

        let chain_changeset = chain.apply_update(chain_update.expect("request has chain tip"))?;

        let mut indexed_tx_graph_changeset =
            indexed_tx_graph::ChangeSet::<ConfirmationBlockTime, _>::default();
        if let Some(keychain_update) = keychain_update {
            let keychain_changeset = graph.index.reveal_to_target_multi(&keychain_update);
            indexed_tx_graph_changeset.merge(keychain_changeset.into());
        }
        indexed_tx_graph_changeset.merge(graph.apply_update(tx_update));

        ChangeSet {
            local_chain: chain_changeset,
            tx_graph: indexed_tx_graph_changeset.tx_graph,
            indexer: indexed_tx_graph_changeset.indexer,
            ..Default::default()
        }
    };

    let mut db = db.lock().unwrap();
    db.append_changeset(&db_changeset)?;
    Ok(())
}
