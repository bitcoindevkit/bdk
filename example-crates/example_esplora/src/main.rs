use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Mutex,
};

use bdk_chain::{
    bitcoin::{Address, Network, OutPoint, Txid},
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::LocalChangeSet,
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

#[derive(Subcommand, Debug, Clone)]
enum EsploraCommands {
    /// Scans the addresses in the wallet sing the esplora API.
    Scan {
        /// When a gap this large has been found for a keychain, it will stop.
        #[clap(long, default_value = "5")]
        stop_gap: usize,
        #[clap(flatten)]
        scan_options: ScanOptions,
    },
    /// Scans particular addresses using the esplora API.
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
    #[clap(long, default_value = "10")]
    pub parallel_requests: usize,
}

fn main() -> anyhow::Result<()> {
    let (args, keymap, index, db, init_changeset) = example_cli::init::<
        EsploraCommands,
        LocalChangeSet<Keychain, ConfirmationTimeAnchor>,
    >(DB_MAGIC, DB_PATH)?;

    let graph = Mutex::new({
        let mut graph = IndexedTxGraph::new(index);
        graph.apply_additions(init_changeset.indexed_additions);
        graph
    });

    let chain = Mutex::new({
        let mut chain = LocalChain::default();
        chain.apply_changeset(&init_changeset.chain_changeset);
        chain
    });

    let esplora_url = match args.network {
        Network::Bitcoin => "https://mempool.space/api",
        Network::Testnet => "https://mempool.space/testnet/api",
        Network::Regtest => "http://localhost:3002",
        Network::Signet => "https://mempool.space/signet/api",
    };

    let client = esplora_client::Builder::new(esplora_url).build_blocking()?;

    // Match the given command. Exectute and return if command is provided by example_cli
    let esplora_cmd = match &args.command {
        // Command that are handled by the specify example
        example_cli::Commands::ChainSpecific(electrum_cmd) => electrum_cmd,
        // General commands handled by example_cli. Execute the cmd and return.
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

    let (update_graph, update_keychain_indices) = match &esplora_cmd {
        EsploraCommands::Scan {
            stop_gap,
            scan_options,
        } => {
            let graph = graph.lock().unwrap();

            let keychain_spks = graph
                .index
                .spks_of_all_keychains()
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

            drop(graph);

            client
                .update_tx_graph(
                    keychain_spks,
                    core::iter::empty(),
                    core::iter::empty(),
                    *stop_gap,
                    scan_options.parallel_requests,
                )
                .context("scanning for transactions")?
        }
        EsploraCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
        } => {
            // Get a short lock on the tracker to get the spks we're interested in
            let graph = graph.lock().unwrap();
            let chain = chain.lock().unwrap();
            let chain_tip = chain.tip().map(|cp| cp.block_id()).unwrap_or_default();

            if !(*all_spks || unused_spks || utxos || unconfirmed) {
                unused_spks = true;
                unconfirmed = true;
                utxos = true;
            } else if *all_spks {
                unused_spks = false;
            }

            let mut spks: Box<dyn Iterator<Item = bdk_chain::bitcoin::Script>> =
                Box::new(core::iter::empty());
            if *all_spks {
                let all_spks = graph
                    .index
                    .all_spks()
                    .iter()
                    .map(|(k, v)| (*k, v.clone()))
                    .collect::<Vec<_>>();
                spks = Box::new(spks.chain(all_spks.into_iter().map(|(index, script)| {
                    eprintln!("scanning {:?}", index);
                    script
                })));
            }
            if unused_spks {
                let unused_spks = graph
                    .index
                    .unused_spks(..)
                    .map(|(k, v)| (*k, v.clone()))
                    .collect::<Vec<_>>();
                spks = Box::new(spks.chain(unused_spks.into_iter().map(|(index, script)| {
                    eprintln!(
                        "Checking if address {} {:?} has been used",
                        Address::from_script(&script, args.network).unwrap(),
                        index
                    );

                    script
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
                    .filter(|canonical_tx| !canonical_tx.observed_as.is_confirmed())
                    .map(|canonical_tx| canonical_tx.node.txid)
                    .collect::<Vec<Txid>>();

                txids = Box::new(unconfirmed_txids.into_iter().inspect(|txid| {
                    eprintln!("Checking if {} is confirmed yet", txid);
                }));
            }

            // drop lock on graph and chain
            drop((graph, chain));

            (
                client
                    .update_tx_graph_without_keychain(
                        spks,
                        txids,
                        outpoints,
                        scan_options.parallel_requests,
                    )
                    .context("syncing transaction updates")?,
                Default::default(),
            )
        }
    };

    println!();

    let (heights_to_fetch, tip) = {
        let chain = &*chain.lock().unwrap();

        let heights_to_fetch = update_graph.missing_blocks(chain).collect::<Vec<_>>();
        let tip = chain.tip();
        (heights_to_fetch, tip)
    };

    #[cfg(debug_assertions)]
    println!(
        "old chain: {:?}",
        tip.iter()
            .flat_map(CheckPoint::iter)
            .map(|cp| cp.height())
            .collect::<Vec<_>>()
    );
    println!("prev tip: {}", tip.as_ref().map_or(0, CheckPoint::height));
    println!("missing blocks: {:?}", heights_to_fetch);

    let update_tip = client
        .update_local_chain(tip, heights_to_fetch)
        .context("scanning for blocks")?;

    #[cfg(debug_assertions)]
    println!(
        "new chain: {:?}",
        update_tip.iter().map(|cp| cp.height()).collect::<Vec<_>>()
    );
    println!("new tip: {}", update_tip.height());

    // check that all anchors are part of the new tip's history
    #[cfg(debug_assertions)]
    {
        use bdk_chain::bitcoin::BlockHash;
        use bdk_chain::collections::HashMap;
        let chain_heights = update_tip
            .iter()
            .map(|cp| (cp.height(), cp.hash()))
            .collect::<HashMap<u32, BlockHash>>();
        for (anchor, _) in update_graph.all_anchors() {
            assert_eq!(anchor.anchor_block.height, anchor.confirmation_height);
            assert!(chain_heights.contains_key(&anchor.anchor_block.height));

            let remote_hash = chain_heights
                .get(&anchor.confirmation_height)
                .expect("must have block");

            // inform about mismatched blocks
            if remote_hash != &anchor.anchor_block.hash {
                println!("mismatched block @ {}!", anchor.confirmation_height);
                println!("\t- anchor_block: {}", anchor.anchor_block.hash);
                println!("\t-   from_chain: {}", remote_hash);
            }
        }
    }

    let db_changeset: LocalChangeSet<Keychain, ConfirmationTimeAnchor> = {
        let mut chain = chain.lock().unwrap();
        let mut graph = graph.lock().unwrap();

        let chain_changeset = chain.apply_update(local_chain::Update {
            tip: update_tip,
            introduce_older_blocks: true,
        })?;

        let indexed_additions = {
            let mut additions = IndexedAdditions::default();
            let (_, index_additions) = graph.index.reveal_to_target_multi(&update_keychain_indices);
            additions.append(IndexedAdditions::from(index_additions));
            additions.append(graph.apply_update(update_graph));
            additions
        };

        LocalChangeSet {
            chain_changeset,
            indexed_additions,
        }
    };

    let mut db = db.lock().unwrap();
    db.stage(db_changeset);
    db.commit()?;
    Ok(())
}
