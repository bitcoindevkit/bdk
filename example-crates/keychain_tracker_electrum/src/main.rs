use bdk_chain::bitcoin::{Address, OutPoint, Txid};
use bdk_electrum::bdk_chain::{self, bitcoin::Network, TxHeight};
use bdk_electrum::{
    electrum_client::{self, ElectrumApi},
    ElectrumExt, ElectrumUpdate,
};
use indexed_tx_graph_example_cli::{
    self as cli,
    anyhow::{self, Context},
    clap::{self, Parser, Subcommand},
};
use std::{collections::BTreeMap, fmt::Debug, io, io::Write};
use std::sync::Mutex;
use bdk_chain::local_chain::LocalChain;

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
    let (args, keymap, indexed_tx_graph/*, db*/) = cli::init::<ElectrumCommands, _>()?;

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
    
    let local_chain = Mutex::new(LocalChain::default());

    let electrum_cmd = match args.command.clone() {
        cli::Commands::ChainSpecific(electrum_cmd) => electrum_cmd,
        general_command => {
            return cli::handle_commands(
                general_command,
                |transaction| {
                    let _txid = client.transaction_broadcast(transaction)?;
                    Ok(())
                },
                &indexed_tx_graph,
//                &db,
                &local_chain,
                args.network,
                &keymap,
            )
        }
    };

    let response = match electrum_cmd {
        ElectrumCommands::Scan {
            stop_gap,
            scan_options: scan_option,
        } => {
            let (keychain_spks, tx_graph) = {
                // Get a short lock on the indexed_tx_graph to get the spks iterators
                // and local chain state
                let locked_indexed_tx_graph = &*indexed_tx_graph.lock().unwrap();
                let spk_iterators = locked_indexed_tx_graph
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
                let tx_graph = locked_indexed_tx_graph.graph().clone();
                (spk_iterators, tx_graph)
            };

            let unlocked_local_chain = *local_chain.lock().unwrap();
            // we scan the spks **without** a lock on the tracker
            client.scan(
                &unlocked_local_chain,
                keychain_spks,
                core::iter::empty(),
                core::iter::empty(),
                stop_gap,
                scan_option.batch_size,
            )?
        }
        ElectrumCommands::Sync {
            mut unused_spks,
            mut utxos,
            mut unconfirmed,
            all_spks,
            scan_options,
        } => {
            // Get a short lock on the tracker to get the spks we're interested in
            let tracker = indexed_tx_graph.lock().unwrap();

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
                let all_spks = tracker
                    .txout_index
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
                let unused_spks = tracker
                    .txout_index
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
                    .full_utxos()
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
                    .chain()
                    .range_txids_by_height(TxHeight::Unconfirmed..)
                    .map(|(_, txid)| *txid)
                    .collect::<Vec<_>>();

                txids = Box::new(unconfirmed_txids.into_iter().inspect(|txid| {
                    eprintln!("Checking if {} is confirmed yet", txid);
                }));
            }

            let local_chain = tracker.chain().checkpoints().clone();
            // drop lock on tracker
            drop(tracker);

            // we scan the spks **without** a lock on the tracker
            ElectrumUpdate {
                chain_update: client
                    .scan_without_keychain(
                        &local_chain,
                        spks,
                        txids,
                        outpoints,
                        scan_options.batch_size,
                    )
                    .context("scanning the blockchain")?,
                ..Default::default()
            }
        }
    };

    let missing_txids = response.missing_full_txs(&*indexed_tx_graph.lock().unwrap());

    // fetch the missing full transactions **without** a lock on the tracker
    let new_txs = client
        .batch_transaction_get(missing_txids)
        .context("fetching full transactions")?;

    {
        // Get a final short lock to apply the changes
        let mut locked_index_tx_graph = indexed_tx_graph.lock().unwrap();
        let changeset = {
            let scan = response.into_keychain_scan(new_txs, &*locked_index_tx_graph)?;
            locked_index_tx_graph.determine_changeset(&scan)?
        };
//        db.lock().unwrap().append_changeset(&changeset)?;
        locked_index_tx_graph.apply_changeset(changeset);
    };

    Ok(())
}
