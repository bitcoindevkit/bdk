use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    sync::Mutex,
};

use bdk_chain::{
    bitcoin::{Address, Network, OutPoint, ScriptBuf, Txid},
    indexed_tx_graph::{self, IndexedTxGraph},
    keychain,
    local_chain::{self, CheckPoint, LocalChain},
    Append, ConfirmationTimeAnchor,
};

use bdk_esplora::{esplora_client, EsploraExt};

use example_cli::{
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
    Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_esplora";
const DB_PATH: &str = ".bdk_esplora_example.db";

type ChangeSet = (
    local_chain::ChangeSet,
    indexed_tx_graph::ChangeSet<ConfirmationTimeAnchor, keychain::ChangeSet<Keychain>>,
);

#[derive(Subcommand, Debug, Clone)]
enum EsploraCommands {
    /// Scans the addresses in the wallet using the esplora API.
    Scan {
        /// When a gap this large has been found for a keychain, it will stop.
        #[clap(long, default_value = "5")]
        stop_gap: usize,
        #[clap(flatten)]
        scan_options: ScanOptions,
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
    },
}

#[derive(Parser, Debug, Clone, PartialEq)]
pub struct ScanOptions {
    /// Max number of concurrent esplora server requests.
    #[clap(long, default_value = "1")]
    pub parallel_requests: usize,
}

fn main() -> anyhow::Result<()> {
    let (args, keymap, index, db, init_changeset) =
        example_cli::init::<EsploraCommands, ChangeSet>(DB_MAGIC, DB_PATH)?;

    let (init_chain_changeset, init_indexed_tx_graph_changeset) = init_changeset;

    // Contruct `IndexedTxGraph` and `LocalChain` with our initial changeset. They are wrapped in
    // `Mutex` to display how they can be used in a multithreaded context. Technically the mutexes
    // aren't strictly needed here.
    let graph = Mutex::new({
        let mut graph = IndexedTxGraph::new(index);
        graph.apply_changeset(init_indexed_tx_graph_changeset);
        graph
    });
    let chain = Mutex::new({
        let mut chain = LocalChain::default();
        chain.apply_changeset(&init_chain_changeset);
        chain
    });

    let esplora_url = match args.network {
        Network::Bitcoin => "https://blockstream.info/api",
        Network::Testnet => "https://blockstream.info/testnet/api",
        Network::Regtest => "http://localhost:3002",
        Network::Signet => "https://mempool.space/signet/api",
        _ => panic!("unsupported network"),
    };

    let client = esplora_client::Builder::new(esplora_url).build_blocking()?;

    let esplora_cmd = match &args.command {
        // These are commands that are handled by this example (sync, scan).
        example_cli::Commands::ChainSpecific(esplora_cmd) => esplora_cmd,
        // These are general commands handled by example_cli. Execute the cmd and return.
        general_cmd => {
            let res = example_cli::handle_commands(
                &graph,
                &db,
                &chain,
                &keymap,
                args.network,
                |tx| {
                    client
                        .broadcast(tx)
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                },
                general_cmd.clone(),
            );

            db.lock().unwrap().commit()?;
            return res;
        }
    };

    // Prepare the `IndexedTxGraph` update based on whether we are scanning or syncing.
    // Scanning: We are iterating through spks of all keychains and scanning for transactions for
    //   each spk. We start with the lowest derivation index spk and stop scanning after `stop_gap`
    //   number of consecutive spks have no transaction history. A Scan is done in situations of
    //   wallet restoration. It is a special case. Applications should use "sync" style updates
    //   after an initial scan.
    // Syncing: We only check for specified spks, utxos and txids to update their confirmation
    //   status or fetch missing transactions.
    let indexed_tx_graph_changeset = match &esplora_cmd {
        EsploraCommands::Scan {
            stop_gap,
            scan_options,
        } => {
            let keychain_spks = graph
                .lock()
                .expect("mutex must not be poisoned")
                .index
                .spks_of_all_keychains()
                .into_iter()
                // This `map` is purely for logging.
                .map(|(keychain, iter)| {
                    let mut first = true;
                    let spk_iter = iter.inspect(move |(i, _)| {
                        if first {
                            eprint!("\nscanning {}: ", keychain);
                            first = false;
                        }
                        eprint!("{} ", i);
                        // Flush early to ensure we print at every iteration.
                        let _ = io::stderr().flush();
                    });
                    (keychain, spk_iter)
                })
                .collect::<BTreeMap<_, _>>();

            // The client scans keychain spks for transaction histories, stopping after `stop_gap`
            // is reached. It returns a `TxGraph` update (`graph_update`) and a structure that
            // represents the last active spk derivation indices of keychains
            // (`keychain_indices_update`).
            let (graph_update, last_active_indices) = client
                .scan_txs_with_keychains(
                    keychain_spks,
                    core::iter::empty(),
                    core::iter::empty(),
                    *stop_gap,
                    scan_options.parallel_requests,
                )
                .context("scanning for transactions")?;

            let mut graph = graph.lock().expect("mutex must not be poisoned");
            // Because we did a stop gap based scan we are likely to have some updates to our
            // deriviation indices. Usually before a scan you are on a fresh wallet with no
            // addresses derived so we need to derive up to last active addresses the scan found
            // before adding the transactions.
            let (_, index_changeset) = graph.index.reveal_to_target_multi(&last_active_indices);
            let mut indexed_tx_graph_changeset = graph.apply_update(graph_update);
            indexed_tx_graph_changeset.append(index_changeset.into());
            indexed_tx_graph_changeset
        }
        EsploraCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
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

            // Spks, outpoints and txids we want updates on will be accumulated here.
            let mut spks: Box<dyn Iterator<Item = ScriptBuf>> = Box::new(core::iter::empty());
            let mut outpoints: Box<dyn Iterator<Item = OutPoint>> = Box::new(core::iter::empty());
            let mut txids: Box<dyn Iterator<Item = Txid>> = Box::new(core::iter::empty());

            // Get a short lock on the structures to get spks, utxos, and txs that we are interested
            // in.
            {
                let graph = graph.lock().unwrap();
                let chain = chain.lock().unwrap();
                let chain_tip = chain.tip().map(|cp| cp.block_id()).unwrap_or_default();

                if *all_spks {
                    let all_spks = graph
                        .index
                        .all_spks()
                        .iter()
                        .map(|(k, v)| (*k, v.clone()))
                        .collect::<Vec<_>>();
                    spks = Box::new(spks.chain(all_spks.into_iter().map(|(index, script)| {
                        eprintln!("scanning {:?}", index);
                        // Flush early to ensure we print at every iteration.
                        let _ = io::stderr().flush();
                        script
                    })));
                }
                if unused_spks {
                    let unused_spks = graph
                        .index
                        .unused_spks(..)
                        .map(|(k, v)| (*k, v.to_owned()))
                        .collect::<Vec<_>>();
                    spks = Box::new(spks.chain(unused_spks.into_iter().map(|(index, script)| {
                        eprintln!(
                            "Checking if address {} {:?} has been used",
                            Address::from_script(&script, args.network).unwrap(),
                            index
                        );
                        // Flush early to ensure we print at every iteration.
                        let _ = io::stderr().flush();
                        script
                    })));
                }
                if utxos {
                    // We want to search for whether the UTXO is spent, and spent by which
                    // transaction. We provide the outpoint of the UTXO to
                    // `EsploraExt::update_tx_graph_without_keychain`.
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
                                // Flush early to ensure we print at every iteration.
                                let _ = io::stderr().flush();
                            })
                            .map(|utxo| utxo.outpoint),
                    );
                };
                if unconfirmed {
                    // We want to search for whether the unconfirmed transaction is now confirmed.
                    // We provide the unconfirmed txids to
                    // `EsploraExt::update_tx_graph_without_keychain`.
                    let unconfirmed_txids = graph
                        .graph()
                        .list_chain_txs(&*chain, chain_tip)
                        .filter(|canonical_tx| !canonical_tx.chain_position.is_confirmed())
                        .map(|canonical_tx| canonical_tx.tx_node.txid)
                        .collect::<Vec<Txid>>();
                    txids = Box::new(unconfirmed_txids.into_iter().inspect(|txid| {
                        eprintln!("Checking if {} is confirmed yet", txid);
                        // Flush early to ensure we print at every iteration.
                        let _ = io::stderr().flush();
                    }));
                }
            }

            let graph_update =
                client.scan_txs(spks, txids, outpoints, scan_options.parallel_requests)?;

            graph.lock().unwrap().apply_update(graph_update)
        }
    };

    println!();

    // Now that we're done updating the `IndexedTxGraph`, it's time to update the `LocalChain`! We
    // want the `LocalChain` to have data about all the anchors in the `TxGraph` - for this reason,
    // we want retrieve the blocks at the heights of the newly added anchors that are missing from
    // our view of the chain.
    let (missing_block_heights, tip) = {
        let chain = &*chain.lock().unwrap();
        let missing_block_heights = indexed_tx_graph_changeset
            .graph
            .missing_heights_from(chain)
            .collect::<BTreeSet<_>>();
        let tip = chain.tip();
        (missing_block_heights, tip)
    };

    println!("prev tip: {}", tip.as_ref().map_or(0, CheckPoint::height));
    println!("missing block heights: {:?}", missing_block_heights);

    // Here, we actually fetch the missing blocks and create a `local_chain::Update`.
    let chain_changeset = {
        let chain_update = client
            .update_local_chain(tip, missing_block_heights)
            .context("scanning for blocks")?;
        println!("new tip: {}", chain_update.tip.height());
        chain.lock().unwrap().apply_update(chain_update)?
    };

    // We persist the changes
    let mut db = db.lock().unwrap();
    db.stage((chain_changeset, indexed_tx_graph_changeset));
    db.commit()?;
    Ok(())
}
