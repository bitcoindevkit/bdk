use core::f32;
use std::{
    collections::BTreeSet,
    io::{self, Write},
};

use bdk_chain::{
    bitcoin::Network,
    keychain_txout::FullScanRequestBuilderExt,
    spk_client::{FullScanRequest, SyncRequest},
    Merge,
};
use bdk_esplora::{esplora_client, EsploraExt};
use example_cli::{
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
    ChangeSet, Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_esplora";
const DB_PATH: &str = ".bdk_example_esplora.db";

#[derive(Subcommand, Debug, Clone)]
enum EsploraCommands {
    /// Scans the addresses in the wallet using the esplora API.
    Scan {
        /// When a gap this large has been found for a keychain, it will stop.
        #[clap(long, short = 'g', default_value = "10")]
        stop_gap: usize,
        #[clap(flatten)]
        scan_options: ScanOptions,
        #[clap(flatten)]
        esplora_args: EsploraArgs,
    },
    /// Scan for particular addresses and unconfirmed transactions using the esplora API.
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
        esplora_args: EsploraArgs,
    },
}

impl EsploraCommands {
    fn esplora_args(&self) -> EsploraArgs {
        match self {
            EsploraCommands::Scan { esplora_args, .. } => esplora_args.clone(),
            EsploraCommands::Sync { esplora_args, .. } => esplora_args.clone(),
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct EsploraArgs {
    /// The esplora url endpoint to connect to.
    #[clap(long, short = 'u', env = "ESPLORA_SERVER")]
    esplora_url: Option<String>,
}

impl EsploraArgs {
    pub fn client(&self, network: Network) -> anyhow::Result<esplora_client::BlockingClient> {
        let esplora_url = self.esplora_url.as_deref().unwrap_or(match network {
            Network::Bitcoin => "https://blockstream.info/api",
            Network::Testnet => "https://blockstream.info/testnet/api",
            Network::Regtest => "http://localhost:3002",
            Network::Signet => "http://signet.bitcoindevkit.net",
            _ => panic!("unsupported network"),
        });

        let client = esplora_client::Builder::new(esplora_url).build_blocking();
        Ok(client)
    }
}

#[derive(Parser, Debug, Clone, PartialEq)]
pub struct ScanOptions {
    /// Max number of concurrent esplora server requests.
    #[clap(long, default_value = "5")]
    pub parallel_requests: usize,
}

fn main() -> anyhow::Result<()> {
    let example_cli::Init {
        args,
        graph,
        chain,
        db,
        network,
    } = match example_cli::init_or_load::<EsploraCommands, EsploraArgs>(DB_MAGIC, DB_PATH)? {
        Some(init) => init,
        None => return Ok(()),
    };

    let esplora_cmd = match &args.command {
        // These are commands that are handled by this example (sync, scan).
        example_cli::Commands::ChainSpecific(esplora_cmd) => esplora_cmd,
        // These are general commands handled by example_cli. Execute the cmd and return.
        general_cmd => {
            return example_cli::handle_commands(
                &graph,
                &chain,
                &db,
                network,
                |esplora_args, tx| {
                    let client = esplora_args.client(network)?;
                    client
                        .broadcast(tx)
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                },
                general_cmd.clone(),
            );
        }
    };

    let client = esplora_cmd.esplora_args().client(network)?;
    // Prepare the `IndexedTxGraph` and `LocalChain` updates based on whether we are scanning or
    // syncing.
    //
    // Scanning: We are iterating through spks of all keychains and scanning for transactions for
    //   each spk. We start with the lowest derivation index spk and stop scanning after `stop_gap`
    //   number of consecutive spks have no transaction history. A Scan is done in situations of
    //   wallet restoration. It is a special case. Applications should use "sync" style updates
    //   after an initial scan.
    //
    // Syncing: We only check for specified spks, utxos and txids to update their confirmation
    //   status or fetch missing transactions.
    let (local_chain_changeset, indexed_tx_graph_changeset) = match &esplora_cmd {
        EsploraCommands::Scan {
            stop_gap,
            scan_options,
            ..
        } => {
            let request = {
                let chain_tip = chain.lock().expect("mutex must not be poisoned").tip();
                let indexed_graph = &*graph.lock().expect("mutex must not be poisoned");
                FullScanRequest::builder_now()
                    .chain_tip(chain_tip)
                    .spks_from_indexer(&indexed_graph.index)
                    .inspect({
                        let mut once = BTreeSet::<Keychain>::new();
                        move |keychain, spk_i, _| {
                            if once.insert(keychain) {
                                eprint!("\nscanning {}: ", keychain);
                            }
                            eprint!("{} ", spk_i);
                            // Flush early to ensure we print at every iteration.
                            let _ = io::stderr().flush();
                        }
                    })
                    .build()
            };

            // The client scans keychain spks for transaction histories, stopping after `stop_gap`
            // is reached. It returns a `TxGraph` update (`tx_update`) and a structure that
            // represents the last active spk derivation indices of keychains
            // (`keychain_indices_update`).
            let update = client
                .full_scan(request, *stop_gap, scan_options.parallel_requests)
                .context("scanning for transactions")?;

            let mut graph = graph.lock().expect("mutex must not be poisoned");
            let mut chain = chain.lock().expect("mutex must not be poisoned");
            // Because we did a stop gap based scan we are likely to have some updates to our
            // deriviation indices. Usually before a scan you are on a fresh wallet with no
            // addresses derived so we need to derive up to last active addresses the scan found
            // before adding the transactions.
            (
                chain.apply_update(update.chain_update.expect("request included chain tip"))?,
                {
                    let index_changeset = graph
                        .index
                        .reveal_to_target_multi(&update.last_active_indices);
                    let mut indexed_tx_graph_changeset = graph.apply_update(update.tx_update);
                    indexed_tx_graph_changeset.merge(index_changeset.into());
                    indexed_tx_graph_changeset
                },
            )
        }
        EsploraCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
            ..
        } => {
            if !(*all_spks || unused_spks || utxos || unconfirmed) {
                // If nothing is specifically selected, we select everything (except all spks).
                unused_spks = true;
                unconfirmed = true;
                utxos = true;
            } else if *all_spks {
                // If all spks is selected, we don't need to also select unused spks (as unused spks
                // is a subset of all spks).
                unused_spks = false;
            }

            let local_tip = chain.lock().expect("mutex must not be poisoned").tip();
            // Spks, outpoints and txids we want updates on will be accumulated here.
            let mut request = SyncRequest::builder_now()
                .chain_tip(local_tip.clone())
                .inspect(|item, progress| {
                    let pc = (100 * progress.consumed()) as f32 / progress.total() as f32;
                    eprintln!("[ SCANNING {:03.0}% ] {}", pc, item);
                    // Flush early to ensure we print at every iteration.
                    let _ = io::stderr().flush();
                });

            // Get a short lock on the structures to get spks, utxos, and txs that we are interested
            // in.
            {
                let graph = graph.lock().unwrap();
                let chain = chain.lock().unwrap();

                if *all_spks {
                    request = request.spks_with_indexes(graph.index.revealed_spks(..));
                }
                if unused_spks {
                    request = request.spks_with_indexes(graph.index.unused_spks());
                }
                if utxos {
                    // We want to search for whether the UTXO is spent, and spent by which
                    // transaction. We provide the outpoint of the UTXO to
                    // `EsploraExt::update_tx_graph_without_keychain`.
                    let init_outpoints = graph.index.outpoints();
                    request = request.outpoints(
                        graph
                            .graph()
                            .filter_chain_unspents(
                                &*chain,
                                local_tip.block_id(),
                                init_outpoints.iter().cloned(),
                            )
                            .map(|(_, utxo)| utxo.outpoint),
                    );
                };
                if unconfirmed {
                    // We want to search for whether the unconfirmed transaction is now confirmed.
                    // We provide the unconfirmed txids to
                    // `EsploraExt::update_tx_graph_without_keychain`.
                    request = request.txids(
                        graph
                            .graph()
                            .list_canonical_txs(&*chain, local_tip.block_id())
                            .filter(|canonical_tx| !canonical_tx.chain_position.is_confirmed())
                            .map(|canonical_tx| canonical_tx.tx_node.txid),
                    );
                }
            }

            let update = client.sync(request, scan_options.parallel_requests)?;

            (
                chain
                    .lock()
                    .unwrap()
                    .apply_update(update.chain_update.expect("request has chain tip"))?,
                graph.lock().unwrap().apply_update(update.tx_update),
            )
        }
    };

    println!();

    // We persist the changes
    let mut db = db.lock().unwrap();
    db.append_changeset(&ChangeSet {
        local_chain: local_chain_changeset,
        tx_graph: indexed_tx_graph_changeset.tx_graph,
        indexer: indexed_tx_graph_changeset.indexer,
        ..Default::default()
    })?;
    Ok(())
}
