use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client, RpcApi},
    EmittedBlock, Emitter, MempoolTx,
};
use bdk_chain::{
    indexed_tx_graph, keychain,
    local_chain::{self, CheckPoint, LocalChain},
    ConfirmationTimeAnchor, IndexedTxGraph,
};
use example_cli::{
    anyhow,
    clap::{self, Args, Subcommand},
    Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_rpc";
const DB_PATH: &str = ".bdk_example_rpc.db";

const CHANNEL_BOUND: usize = 10;
/// Delay (in seconds) for mempool emissions and printing status to stdout.
const LIVE_POLL_DELAY_SEC: Duration = Duration::from_secs(15);

/// The block depth which we assume no reorgs can happen at.
const ASSUME_FINAL_DEPTH: u32 = 6;
const DB_COMMIT_DELAY_SEC: Duration = Duration::from_secs(30);

type ChangeSet = (
    local_chain::ChangeSet,
    indexed_tx_graph::ChangeSet<ConfirmationTimeAnchor, keychain::ChangeSet<Keychain>>,
);

#[derive(Debug)]
enum Emission {
    Block(EmittedBlock),
    Mempool(Vec<MempoolTx>),
    Tip(u32),
}

#[derive(Args, Debug, Clone)]
struct RpcArgs {
    /// RPC URL
    #[clap(env = "RPC_URL", long, default_value = "127.0.0.1:8332")]
    url: String,
    /// RPC auth cookie file
    #[clap(env = "RPC_COOKIE", long)]
    rpc_cookie: Option<PathBuf>,
    /// RPC auth username
    #[clap(env = "RPC_USER", long)]
    rpc_user: Option<String>,
    /// RPC auth password
    #[clap(env = "RPC_PASS", long)]
    rpc_password: Option<String>,
    /// Starting block height to fallback to if no point of agreement if found
    #[clap(env = "FALLBACK_HEIGHT", long, default_value = "0")]
    fallback_height: u32,
    /// The unused-scripts lookahead will be kept at this size
    #[clap(long, default_value = "10")]
    lookahead: u32,
}

impl From<RpcArgs> for Auth {
    fn from(args: RpcArgs) -> Self {
        match (args.rpc_cookie, args.rpc_user, args.rpc_password) {
            (None, None, None) => Self::None,
            (Some(path), _, _) => Self::CookieFile(path),
            (_, Some(user), Some(pass)) => Self::UserPass(user, pass),
            (_, Some(_), None) => panic!("rpc auth: missing rpc_pass"),
            (_, None, Some(_)) => panic!("rpc auth: missing rpc_user"),
        }
    }
}

impl RpcArgs {
    fn new_client(&self) -> anyhow::Result<Client> {
        Ok(Client::new(
            &self.url,
            match (&self.rpc_cookie, &self.rpc_user, &self.rpc_password) {
                (None, None, None) => Auth::None,
                (Some(path), _, _) => Auth::CookieFile(path.clone()),
                (_, Some(user), Some(pass)) => Auth::UserPass(user.clone(), pass.clone()),
                (_, Some(_), None) => panic!("rpc auth: missing rpc_pass"),
                (_, None, Some(_)) => panic!("rpc auth: missing rpc_user"),
            },
        )?)
    }
}

#[derive(Subcommand, Debug, Clone)]
enum RpcCommands {
    /// Syncs local state with remote state via RPC (starting from last point of agreement) and
    /// stores/indexes relevant transactions
    Sync {
        #[clap(flatten)]
        rpc_args: RpcArgs,
    },
    Live {
        #[clap(flatten)]
        rpc_args: RpcArgs,
    },
}

fn main() -> anyhow::Result<()> {
    let (args, keymap, index, db, init_changeset) =
        example_cli::init::<RpcCommands, RpcArgs, ChangeSet>(DB_MAGIC, DB_PATH)?;

    let graph = Mutex::new({
        let mut graph = IndexedTxGraph::new(index);
        graph.apply_changeset(init_changeset.1);
        graph
    });

    let chain = Mutex::new(LocalChain::from_changeset(init_changeset.0));

    let rpc_cmd = match args.command {
        example_cli::Commands::ChainSpecific(rpc_cmd) => rpc_cmd,
        general_cmd => {
            let res = example_cli::handle_commands(
                &graph,
                &db,
                &chain,
                &keymap,
                args.network,
                |rpc_args, tx| {
                    let client = rpc_args.new_client()?;
                    client.send_raw_transaction(tx)?;
                    Ok(())
                },
                general_cmd,
            );
            db.lock().unwrap().commit()?;
            return res;
        }
    };

    match rpc_cmd {
        RpcCommands::Sync { rpc_args } => {
            let RpcArgs {
                fallback_height,
                lookahead,
                ..
            } = rpc_args;

            let mut chain = chain.lock().unwrap();
            let mut graph = graph.lock().unwrap();
            let mut db = db.lock().unwrap();

            graph.index.set_lookahead_for_all(lookahead);

            // we start at a height lower than last-seen tip in case of reorgs
            let prev_cp = chain.tip();
            let start_height = prev_cp.as_ref().map_or(fallback_height, |cp| {
                cp.height().saturating_sub(ASSUME_FINAL_DEPTH)
            });

            let rpc_client = rpc_args.new_client()?;
            let mut emitter = Emitter::new(&rpc_client, start_height);

            let mut last_db_commit = Instant::now();

            while let Some(block) = emitter.next_block()? {
                let chain_update =
                    CheckPoint::from_header(&block.block.header, block.height).into_update(false);
                let chain_changeset = chain.apply_update(chain_update)?;
                let graph_changeset = graph.apply_block(block.block, block.height);

                db.stage((chain_changeset, graph_changeset));
                if last_db_commit.elapsed() >= DB_COMMIT_DELAY_SEC {
                    db.commit()?;
                    last_db_commit = Instant::now();
                }
            }

            // mempool
            let mempool_txs = emitter.mempool()?;
            let graph_changeset = graph.batch_insert_unconfirmed(
                mempool_txs.iter().map(|m_tx| (&m_tx.tx, Some(m_tx.time))),
            );
            db.stage((local_chain::ChangeSet::default(), graph_changeset));

            // commit one last time!
            db.commit()?;
        }
        RpcCommands::Live { rpc_args } => {
            let RpcArgs {
                fallback_height,
                lookahead,
                ..
            } = rpc_args;
            let sigterm_flag = start_ctrlc_handler();

            graph.lock().unwrap().index.set_lookahead_for_all(lookahead);
            let start_height = chain.lock().unwrap().tip().map_or(fallback_height, |cp| {
                cp.height().saturating_sub(ASSUME_FINAL_DEPTH)
            });

            let (tx, rx) = std::sync::mpsc::sync_channel::<Emission>(CHANNEL_BOUND);
            let emission_jh = std::thread::spawn(move || -> anyhow::Result<()> {
                let rpc_client = rpc_args.new_client()?;
                let mut emitter = Emitter::new(&rpc_client, start_height);
                let mut last_mempool_update = Instant::now();

                let mut block_count = rpc_client.get_block_count()? as u32;
                tx.send(Emission::Tip(block_count))?;

                loop {
                    match emitter.next_block()? {
                        Some(block) => {
                            if block.height > block_count {
                                block_count = rpc_client.get_block_count()? as u32;
                                tx.send(Emission::Tip(block_count))?;
                            }
                            tx.send(Emission::Block(block))?;
                            if last_mempool_update.elapsed() >= LIVE_POLL_DELAY_SEC {
                                last_mempool_update = Instant::now();
                                tx.send(Emission::Mempool(emitter.mempool()?))?;
                            }
                        }
                        None => {
                            if await_flag(&sigterm_flag, LIVE_POLL_DELAY_SEC) {
                                return Ok(());
                            }
                            last_mempool_update = Instant::now();
                            tx.send(Emission::Mempool(emitter.mempool()?))?;
                            continue;
                        }
                    };
                }
            });

            let mut last_db_commit = Instant::now();
            let mut db = db.lock().unwrap();
            let mut graph = graph.lock().unwrap();
            let mut chain = chain.lock().unwrap();
            let mut tip_height = 0_u32;

            for emission in rx {
                let changeset = match emission {
                    Emission::Block(block) => {
                        let chain_update =
                            CheckPoint::from_header(&block.block.header, block.height)
                                .into_update(false);
                        let chain_changeset = chain.apply_update(chain_update)?;
                        let graph_changeset = graph.apply_block(block.block, block.height);
                        (chain_changeset, graph_changeset)
                    }
                    Emission::Mempool(mempool_txs) => {
                        let graph_changeset = graph.batch_insert_unconfirmed(
                            mempool_txs.iter().map(|m_tx| (&m_tx.tx, Some(m_tx.time))),
                        );
                        (local_chain::ChangeSet::default(), graph_changeset)
                    }
                    Emission::Tip(h) => {
                        tip_height = h;
                        continue;
                    }
                };

                db.stage(changeset);

                if last_db_commit.elapsed() >= DB_COMMIT_DELAY_SEC {
                    if let Some(synced_to) = chain.tip() {
                        let balance = {
                            graph.graph().balance(
                                &*chain,
                                synced_to.block_id(),
                                graph.index.outpoints().iter().cloned(),
                                |(k, _), _| k == &Keychain::Internal,
                            )
                        };
                        println!(
                            "[status] synced to {} @ {} / {} | total: {} sats",
                            synced_to.hash(),
                            synced_to.height(),
                            tip_height,
                            balance.total()
                        );
                    }

                    db.commit()?;
                    last_db_commit = Instant::now();
                    println!("[status] commited to db");
                }
            }

            emission_jh.join().expect("must join emitter thread")?;
        }
    }

    Ok(())
}

#[allow(dead_code)]
fn start_ctrlc_handler() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let cloned_flag = flag.clone();

    ctrlc::set_handler(move || cloned_flag.store(true, Ordering::Release));

    flag
}

#[allow(dead_code)]
fn await_flag(flag: &AtomicBool, duration: Duration) -> bool {
    let start = Instant::now();
    loop {
        if flag.load(Ordering::Acquire) {
            return true;
        }
        if start.elapsed() >= duration {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
