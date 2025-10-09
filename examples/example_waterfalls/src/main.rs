use std::{
    collections::BTreeSet,
    io::{self, Write},
};

use bdk_chain::{
    bitcoin::Network, keychain_txout::FullScanRequestBuilderExt, spk_client::FullScanRequest, Merge,
};
use example_cli::{
    anyhow::{self},
    clap::{self, Parser, Subcommand},
    ChangeSet, Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_waterfalls";
const DB_PATH: &str = ".bdk_example_waterfalls.db";

#[derive(Subcommand, Debug, Clone)]
enum WaterfallsCommands {
    /// Scans the addresses in the wallet using the waterfalls API.
    Scan {
        #[clap(flatten)]
        waterfalls_args: WaterfallsArgs,

        #[clap(flatten)]
        scan_options: ScanOptions,
    },
}

impl WaterfallsCommands {
    fn waterfalls_args(&self) -> WaterfallsArgs {
        match self {
            WaterfallsCommands::Scan {
                waterfalls_args, ..
            } => waterfalls_args.clone(),
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
pub struct WaterfallsArgs {
    /// The esplora url endpoint to connect to.
    #[clap(long, short = 'u', env = "WATERFALLS_SERVER")]
    waterfalls_url: Option<String>,
}

#[derive(Parser, Debug, Clone, PartialEq)]
pub struct ScanOptions {
    /// Max number of concurrent esplora server requests.
    #[clap(long, default_value = "2")]
    pub parallel_requests: usize,
}

impl WaterfallsArgs {
    pub fn client(&self, network: Network) -> anyhow::Result<bdk_waterfalls::Client> {
        let waterfalls_url = self.waterfalls_url.as_deref().unwrap_or(match network {
            Network::Bitcoin => "https://waterfalls.liquidwebwallet.org/bitcoin/api",
            Network::Regtest => "http://localhost:3002",
            Network::Signet => "https://waterfalls.liquidwebwallet.org/bitcoinsignet/api",
            _ => panic!("unsupported network"),
        });

        let client = bdk_waterfalls::Client::new(network, waterfalls_url).unwrap(); // TODO: handle error
        Ok(client)
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let example_cli::Init {
        args,
        graph,
        chain,
        db,
        network,
    } = match example_cli::init_or_load::<WaterfallsCommands, WaterfallsArgs>(DB_MAGIC, DB_PATH)? {
        Some(init) => init,
        None => return Ok(()),
    };

    let waterfalls_cmd = match &args.command {
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
                    client.broadcast(tx).unwrap(); // TODO: handle error
                    Ok(())
                },
                general_cmd.clone(),
            );
        }
    };

    let client = waterfalls_cmd.waterfalls_args().client(network)?;
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
    let (local_chain_changeset, indexed_tx_graph_changeset) = match &waterfalls_cmd {
        WaterfallsCommands::Scan { scan_options, .. } => {
            let request = {
                let chain_tip = chain.lock().expect("mutex must not be poisoned").tip();
                let indexed_graph = &*graph.lock().expect("mutex must not be poisoned");
                FullScanRequest::builder()
                    .chain_tip(chain_tip)
                    .spks_from_indexer(&indexed_graph.index)
                    .inspect({
                        let mut once = BTreeSet::<Keychain>::new();
                        move |keychain, spk_i, _| {
                            if once.insert(keychain) {
                                eprint!("\nscanning {keychain}: ");
                            }
                            eprint!("{spk_i} ");
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
                .full_scan(request, 10, scan_options.parallel_requests)
                .unwrap(); // TODO: handle error

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
    };

    println!();

    // We persist the changes
    let mut db = db.lock().unwrap();
    db.append(&ChangeSet {
        local_chain: local_chain_changeset,
        tx_graph: indexed_tx_graph_changeset.tx_graph,
        indexer: indexed_tx_graph_changeset.indexer,
        ..Default::default()
    })?;
    Ok(())
}
