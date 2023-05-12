use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Mutex,
};

use bdk_chain::{
    bitcoin::{Address, BlockHash, Network, OutPoint, Txid},
    indexed_tx_graph::IndexedAdditions,
    local_chain::{self, LocalChain},
    Append, ConfirmationHeightAnchor,
};
use bdk_electrum::{
    electrum_client::{self, ElectrumApi},
    v2::{ElectrumExt, ElectrumUpdate},
};
use example_cli::{
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
};

const DB_MAGIC: &[u8] = b"bdk_example_electrum";
const DB_PATH: &str = ".bdk_electrum_example.db";
const ASSUME_FINAL_DEPTH: usize = 10;

#[derive(Subcommand, Debug, Clone)]
enum ElectrumCommands {
    /// Scans the addresses in the wallet using the esplora API.
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
    /// Set batch size for each script_history call to electrum client.
    #[clap(long, default_value = "25")]
    pub batch_size: usize,
}

fn main() -> anyhow::Result<()> {
    let (args, keymap, graph, db, chain_changeset) =
        example_cli::init::<ElectrumCommands, ConfirmationHeightAnchor, local_chain::ChangeSet>(
            DB_MAGIC, DB_PATH,
        )?;

    let chain = Mutex::new({
        let mut chain = LocalChain::default();
        chain.apply_changeset(chain_changeset);
        chain
    });

    let electrum_url = match args.network {
        Network::Bitcoin => "ssl://electrum.blockstream.info:50002",
        Network::Testnet => "ssl://electrum.blockstream.info:60002",
        Network::Regtest => "tcp://localhost:60401",
        Network::Signet => "tcp://signet-electrumx.wakiyamap.dev:50001",
    };
    let config = electrum_client::Config::builder()
        .validate_domain(matches!(args.network, Network::Bitcoin))
        .build();

    let client = electrum_client::Client::from_config(electrum_url, config)?;

    let electrum_cmd = match &args.command {
        example_cli::Commands::ChainSpecific(electrum_cmd) => electrum_cmd,
        general_cmd => {
            let res = example_cli::handle_commands(
                &graph,
                &db,
                &chain,
                &keymap,
                args.network,
                |tx| {
                    client
                        .transaction_broadcast(tx)
                        .map(|_| ())
                        .map_err(anyhow::Error::from)
                },
                general_cmd.clone(),
            );

            db.lock().unwrap().commit()?;
            return res;
        }
    };

    let response = match electrum_cmd.clone() {
        ElectrumCommands::Scan {
            stop_gap,
            scan_options,
        } => {
            let (keychain_spks, c) = {
                let graph = &*graph.lock().unwrap();
                let chain = &*chain.lock().unwrap();

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

                let c = chain
                    .blocks()
                    .iter()
                    .rev()
                    .take(ASSUME_FINAL_DEPTH)
                    .map(|(k, v)| (*k, *v))
                    .collect::<BTreeMap<u32, BlockHash>>();

                (keychain_spks, c)
            };

            client
                .scan(
                    &c,
                    keychain_spks,
                    core::iter::empty(),
                    core::iter::empty(),
                    stop_gap,
                    scan_options.batch_size,
                )
                .context("scanning the blockchain")?
        }
        ElectrumCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
        } => {
            // Get a short lock on the tracker to get the spks we're interested in
            let graph = graph.lock().unwrap();
            let chain = chain.lock().unwrap();
            let chain_tip = chain.tip().unwrap_or_default();

            if !(all_spks || unused_spks || utxos || unconfirmed) {
                unused_spks = true;
                unconfirmed = true;
                utxos = true;
            } else if all_spks {
                unused_spks = false;
            }

            let mut spks: Box<dyn Iterator<Item = bdk_chain::bitcoin::Script>> =
                Box::new(core::iter::empty());
            if all_spks {
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

            let c = chain
                .blocks()
                .iter()
                .rev()
                .take(ASSUME_FINAL_DEPTH)
                .map(|(k, v)| (*k, *v))
                .collect::<BTreeMap<u32, BlockHash>>();

            // drop lock on graph and chain
            drop((graph, chain));

            let update = client
                .scan_without_keychain(&c, spks, txids, outpoints, scan_options.batch_size)
                .context("scanning the blockchain")?;
            ElectrumUpdate {
                graph_update: update.graph_update,
                chain_update: update.chain_update,
                keychain_update: BTreeMap::new(),
            }
        }
    };

    let missing_txids = {
        let graph = &*graph.lock().unwrap();
        response
            .missing_full_txs(graph.graph())
            .cloned()
            .collect::<Vec<_>>()
    };

    let new_txs = client
        .batch_transaction_get(&missing_txids)
        .context("fetching full transactions")?;
    let now = std::time::UNIX_EPOCH
        .elapsed()
        .expect("must get time")
        .as_secs();
    let final_update = response.finalize(Some(now), new_txs);

    let db_changeset = {
        let mut chain = chain.lock().unwrap();
        let mut graph = graph.lock().unwrap();

        let chain_changeset = chain.apply_update(final_update.chain)?;

        let indexed_additions = {
            let mut additions = IndexedAdditions::<ConfirmationHeightAnchor, _>::default();
            let (_, index_additions) = graph.index.reveal_to_target_multi(&final_update.keychain);
            additions.append(IndexedAdditions {
                index_additions,
                ..Default::default()
            });
            additions.append(graph.apply_update(final_update.graph));
            additions
        };

        example_cli::ChangeSet {
            indexed_additions,
            extension: chain_changeset,
        }
    };

    let mut db = db.lock().unwrap();
    db.stage(db_changeset);
    db.commit()?;
    Ok(())
}
