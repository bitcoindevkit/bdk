use std::{
    collections::BTreeMap,
    io::{self, Write},
    time::UNIX_EPOCH,
};

use bdk_chain::{
    bitcoin::{Address, BlockHash, Network, OutPoint, Txid},
    Append, ConfirmationHeightAnchor,
};
use bdk_electrum::{
    electrum_client::{self, ElectrumApi},
    v2::{ElectrumExt, ElectrumUpdate},
};
use tracker_example_cli::{
    self as cli,
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
};

const DB_MAGIC: &[u8] = b"bdk_example_electrum";
const DB_PATH: &str = ".bdk_electrum_example.db";

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
    let (args, keymap, tracker, db) = cli::init::<ElectrumCommands, ConfirmationHeightAnchor, _, _>(
        DB_MAGIC,
        DB_PATH,
        cli::Tracker::new_local(),
    )?;

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

    // [TODO]: Use genesis block based on network!
    let chain_tip = tracker.lock().unwrap().chain.tip().unwrap_or_default();

    let electrum_cmd = match args.command.clone() {
        cli::Commands::ChainSpecific(electrum_cmd) => electrum_cmd,
        general_command => {
            return cli::handle_commands(
                general_command,
                |transaction| {
                    let _txid = client.transaction_broadcast(transaction)?;
                    Ok(())
                },
                &tracker,
                &db,
                chain_tip,
                args.network,
                &keymap,
            )
        }
    };

    let response = match electrum_cmd {
        ElectrumCommands::Scan {
            stop_gap,
            scan_options,
        } => {
            let (spk_iters, local_chain) = {
                let tracker = &*tracker.lock().unwrap();
                let spk_iters = tracker
                    .indexed_graph
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
                let local_chain: BTreeMap<u32, BlockHash> = tracker.chain.clone().into();
                (spk_iters, local_chain)
            };

            client.scan(
                &local_chain,
                spk_iters,
                core::iter::empty(),
                core::iter::empty(),
                stop_gap,
                scan_options.batch_size,
            )?
        }
        ElectrumCommands::Sync {
            mut unused_spks,
            all_spks,
            mut utxos,
            mut unconfirmed,
            scan_options,
        } => {
            // Get a short lock on the tracker to get the spks we're interested in
            let tracker = tracker.lock().unwrap();

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
                let index = &tracker.indexed_graph.index;
                let all_spks = index
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
                let index = &tracker.indexed_graph.index;
                let unused_spks = index
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
                let utxos = tracker
                    .list_owned_unspents(chain_tip)
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
                let unconfirmed_txids = tracker
                    .list_txs(chain_tip)
                    .filter(|ctx| !ctx.observed_as.is_confirmed())
                    .map(|ctx| ctx.node.txid)
                    .collect::<Vec<_>>();

                txids = Box::new(unconfirmed_txids.into_iter().inspect(|txid| {
                    eprintln!("Checking if {} is confirmed yet", txid);
                }));
            }

            let local_chain: BTreeMap<u32, BlockHash> = tracker.chain.clone().into();
            drop(tracker);

            let update = client.scan_without_keychain(
                &local_chain,
                spks,
                txids,
                outpoints,
                scan_options.batch_size,
            )?;
            ElectrumUpdate {
                graph_update: update.graph_update,
                chain_update: update.chain_update,
                keychain_update: BTreeMap::new(),
            }
        }
    };
    println!();

    let missing_txids = {
        let tracker = &*tracker.lock().unwrap();
        response
            .missing_full_txs(tracker.indexed_graph.graph())
            .cloned()
            .collect::<Vec<_>>()
    };

    let update = response.finalize(
        UNIX_EPOCH.elapsed().map(|d| d.as_secs()).ok(),
        client
            .batch_transaction_get(&missing_txids)
            .context("fetching full transactions")?,
    );

    {
        use bdk_chain::PersistBackend;
        let tracker = &mut *tracker.lock().unwrap();
        let db = &mut *db.lock().unwrap();

        let (additions, changeset) =
            update.apply(&mut tracker.indexed_graph, &mut tracker.chain)?;

        let mut tracker_changeset = cli::ChangeSet::default();
        tracker_changeset.append(additions.into());
        tracker_changeset.append(changeset.into());

        // [TODO] How do we check if changeset is empty?
        // [TODO] When should we flush?
        db.write_changes(&tracker_changeset)?;
    }

    Ok(())
}
