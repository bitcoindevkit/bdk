use std::sync::Mutex;

use bdk_cbf::{CBFClient, Network};
use bdk_chain::{keychain::LocalChangeSet, ConfirmationHeightAnchor, IndexedTxGraph};
use example_cli::{
    anyhow,
    clap::{self, Args, Subcommand},
    Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_cbf";
const DB_PATH: &str = ".bdk_example_cbf.db";

type ChangeSet = LocalChangeSet<Keychain, ConfirmationHeightAnchor>;

#[derive(Debug, Clone, Args)]
struct CBFArgs {}

#[derive(Subcommand, Debug, Clone)]
enum CBFCommands {
    Scan {
        /// The block height to start scanning from
        #[clap(long, default_value = "0")]
        start_height: u64,
        /// The block height to stop scanning at
        #[clap(long, default_value = "5")]
        stop_gap: u32,
        /// Number of scripts to watch for every sync
        #[clap(long, default_value = "1000")]
        watchlist_size: u32,
    },
}

fn main() -> anyhow::Result<()> {
    let (args, keymap, index, db, init_changeset) =
        example_cli::init::<CBFCommands, ChangeSet>(DB_MAGIC, DB_PATH)?;

    let graph = Mutex::new({
        let mut graph = IndexedTxGraph::new(index);
        graph.apply_additions(init_changeset.indexed_additions);
        graph
    });

    let client = Mutex::new({
        let client = CBFClient::start_client(Network::Testnet, 1)?;
        client
    });

    let cbf_cmd = match args.command {
        example_cli::Commands::ChainSpecific(cbf_cmd) => cbf_cmd,
        general_cmd => {
            let res = example_cli::handle_commands(
                &graph,
                &db,
                &client,
                &keymap,
                args.network,
                |tx| {
                    client
                        .lock()
                        .unwrap()
                        .submit_transaction(tx.clone())
                        .map_err(anyhow::Error::from)
                },
                general_cmd,
            );
            db.lock().unwrap().commit()?;
            return res;
        }
    };

    match cbf_cmd {
        CBFCommands::Scan {
            start_height,
            stop_gap,
            watchlist_size,
        } => {
            println!("Scanning from height {} to {}", start_height, stop_gap);
            let indexed_additions = {
                let mut graph = graph.lock().unwrap();
                client
                    .lock()
                    .unwrap()
                    .scan(watchlist_size, start_height, &mut graph, stop_gap)?
            };

            let curr_changeset = LocalChangeSet::from(indexed_additions);

            // stage changes to the database
            let mut db = db.lock().unwrap();
            db.stage(curr_changeset);
            db.commit()?;

            println!("commited to database!");
        }
    }

    Ok(())
}
