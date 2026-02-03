use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client, RpcApi},
    Emitter,
};
use bdk_chain::{bitcoin::Block, local_chain, CanonicalizationParams, Merge};
use example_cli::{
    anyhow,
    clap::{self, Args, Subcommand},
    ChangeSet, Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_rpc";
const DB_PATH: &str = ".bdk_example_rpc.db";

/// The mpsc channel bound for emissions from [`Emitter`].
const CHANNEL_BOUND: usize = 10;
/// Delay for printing status to stdout.
const STDOUT_PRINT_DELAY: Duration = Duration::from_secs(6);
/// Delay between mempool emissions.
const MEMPOOL_EMIT_DELAY: Duration = Duration::from_secs(30);
/// Delay for committing to persistence.
const DB_COMMIT_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug)]
enum Emission {
    Block(bdk_bitcoind_rpc::BlockEvent<Block>),
    Mempool(bdk_bitcoind_rpc::MempoolEvent),
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
    /// Sync by having the emitter logic in a separate thread
    Live {
        #[clap(flatten)]
        rpc_args: RpcArgs,
    },
}

fn main() -> anyhow::Result<()> {
    let start = Instant::now();

    let example_cli::Init {
        args,
        graph,
        chain,
        db,
        network,
    } = match example_cli::init_or_load::<RpcCommands, RpcArgs>(DB_MAGIC, DB_PATH)? {
        Some(init) => init,
        None => return Ok(()),
    };

    let rpc_cmd = match args.command {
        example_cli::Commands::ChainSpecific(rpc_cmd) => rpc_cmd,
        general_cmd => {
            return example_cli::handle_commands(
                &graph,
                &chain,
                &db,
                network,
                |rpc_args, tx| {
                    let client = rpc_args.new_client()?;
                    client.send_raw_transaction(tx)?;
                    Ok(())
                },
                general_cmd,
            );
        }
    };

    match rpc_cmd {
        RpcCommands::Sync { rpc_args } => {
            let RpcArgs {
                fallback_height, ..
            } = rpc_args;

            let rpc_client = rpc_args.new_client()?;
            let mut emitter = {
                let chain = chain.lock().unwrap();
                let graph = graph.lock().unwrap();
                Emitter::new(
                    &rpc_client,
                    chain.tip(),
                    fallback_height,
                    {
                        let chain_tip = chain.tip().block_id();
                        let task = graph
                            .graph()
                            .canonicalization_task(chain_tip, CanonicalizationParams::default());
                        chain.canonicalize(task)
                    }
                    .txs()
                    .filter(|tx| tx.pos.is_unconfirmed())
                    .map(|tx| tx.tx),
                )
            };
            let mut db_stage = ChangeSet::default();

            let mut last_db_commit = Instant::now();
            let mut last_print = Instant::now();

            while let Some(emission) = emitter.next_block()? {
                let height = emission.block_height();

                let mut chain = chain.lock().unwrap();
                let mut graph = graph.lock().unwrap();

                let chain_changeset = chain
                    .apply_update(emission.checkpoint)
                    .expect("must always apply as we receive blocks in order from emitter");
                let graph_changeset = graph.apply_block_relevant(&emission.block, height);
                db_stage.merge(ChangeSet {
                    local_chain: chain_changeset,
                    tx_graph: graph_changeset.tx_graph,
                    indexer: graph_changeset.indexer,
                    ..Default::default()
                });

                // commit staged db changes in intervals
                if last_db_commit.elapsed() >= DB_COMMIT_DELAY {
                    let db = &mut *db.lock().unwrap();
                    last_db_commit = Instant::now();
                    if let Some(changeset) = db_stage.take() {
                        db.append(&changeset)?;
                    }
                    println!(
                        "[{:>10}s] committed to db (took {}s)",
                        start.elapsed().as_secs_f32(),
                        last_db_commit.elapsed().as_secs_f32()
                    );
                }

                // print synced-to height and current balance in intervals
                if last_print.elapsed() >= STDOUT_PRINT_DELAY {
                    last_print = Instant::now();
                    let synced_to = chain.tip();
                    let balance = {
                        {
                            let synced_to_block = synced_to.block_id();
                            let task = graph.graph().canonicalization_task(
                                synced_to_block,
                                CanonicalizationParams::default(),
                            );
                            chain.canonicalize(task)
                        }
                        .balance(
                            graph.index.outpoints().iter().cloned(),
                            |(k, _), _| k == &Keychain::Internal,
                            0,
                        )
                    };
                    println!(
                        "[{:>10}s] synced to {} @ {} | total: {}",
                        start.elapsed().as_secs_f32(),
                        synced_to.hash(),
                        synced_to.height(),
                        balance.total()
                    );
                }
            }

            let mempool_txs = emitter.mempool()?;
            let graph_changeset = graph
                .lock()
                .unwrap()
                .batch_insert_relevant_unconfirmed(mempool_txs.update);
            {
                let db = &mut *db.lock().unwrap();
                db_stage.merge(ChangeSet {
                    tx_graph: graph_changeset.tx_graph,
                    indexer: graph_changeset.indexer,
                    ..Default::default()
                });
                if let Some(changeset) = db_stage.take() {
                    db.append(&changeset)?;
                }
            }
        }
        RpcCommands::Live { rpc_args } => {
            let RpcArgs {
                fallback_height, ..
            } = rpc_args;
            let sigterm_flag = start_ctrlc_handler();

            let rpc_client = Arc::new(rpc_args.new_client()?);
            let mut emitter = {
                let chain = chain.lock().unwrap();
                let graph = graph.lock().unwrap();
                Emitter::new(
                    rpc_client.clone(),
                    chain.tip(),
                    fallback_height,
                    {
                        let chain_tip = chain.tip().block_id();
                        let task = graph
                            .graph()
                            .canonicalization_task(chain_tip, CanonicalizationParams::default());
                        chain.canonicalize(task)
                    }
                    .txs()
                    .filter(|tx| tx.pos.is_unconfirmed())
                    .map(|tx| tx.tx),
                )
            };

            println!(
                "[{:>10}s] starting emitter thread...",
                start.elapsed().as_secs_f32()
            );
            let (tx, rx) = std::sync::mpsc::sync_channel::<Emission>(CHANNEL_BOUND);
            let emission_jh = std::thread::spawn(move || -> anyhow::Result<()> {
                let mut block_count = rpc_client.get_block_count()? as u32;
                tx.send(Emission::Tip(block_count))?;

                loop {
                    match emitter.next_block()? {
                        Some(block_emission) => {
                            let height = block_emission.block_height();
                            if sigterm_flag.load(Ordering::Acquire) {
                                break;
                            }
                            if height > block_count {
                                block_count = rpc_client.get_block_count()? as u32;
                                tx.send(Emission::Tip(block_count))?;
                            }
                            tx.send(Emission::Block(block_emission))?;
                        }
                        None => {
                            if await_flag(&sigterm_flag, MEMPOOL_EMIT_DELAY) {
                                break;
                            }
                            println!("preparing mempool emission...");
                            let now = Instant::now();
                            tx.send(Emission::Mempool(emitter.mempool()?))?;
                            println!("mempool emission prepared in {}s", now.elapsed().as_secs());
                            continue;
                        }
                    };
                }

                println!("emitter thread shutting down...");
                Ok(())
            });

            let mut tip_height = 0_u32;
            let mut last_db_commit = Instant::now();
            let mut last_print = Option::<Instant>::None;
            let mut db_stage = ChangeSet::default();

            for emission in rx {
                let mut graph = graph.lock().unwrap();
                let mut chain = chain.lock().unwrap();

                let (chain_changeset, graph_changeset) = match emission {
                    Emission::Block(block_emission) => {
                        let height = block_emission.block_height();
                        let chain_changeset = chain
                            .apply_update(block_emission.checkpoint)
                            .expect("must always apply as we receive blocks in order from emitter");
                        let graph_changeset =
                            graph.apply_block_relevant(&block_emission.block, height);
                        (chain_changeset, graph_changeset)
                    }
                    Emission::Mempool(mempool_txs) => {
                        let mut graph_changeset =
                            graph.batch_insert_relevant_unconfirmed(mempool_txs.update.clone());
                        graph_changeset
                            .merge(graph.batch_insert_relevant_evicted_at(mempool_txs.evicted));
                        (local_chain::ChangeSet::default(), graph_changeset)
                    }
                    Emission::Tip(h) => {
                        tip_height = h;
                        continue;
                    }
                };

                db_stage.merge(ChangeSet {
                    local_chain: chain_changeset,
                    tx_graph: graph_changeset.tx_graph,
                    indexer: graph_changeset.indexer,
                    ..Default::default()
                });

                if last_db_commit.elapsed() >= DB_COMMIT_DELAY {
                    let db = &mut *db.lock().unwrap();
                    last_db_commit = Instant::now();
                    if let Some(changeset) = db_stage.take() {
                        db.append(&changeset)?;
                    }
                    println!(
                        "[{:>10}s] committed to db (took {}s)",
                        start.elapsed().as_secs_f32(),
                        last_db_commit.elapsed().as_secs_f32()
                    );
                }

                if last_print.map_or(Duration::MAX, |i| i.elapsed()) >= STDOUT_PRINT_DELAY {
                    last_print = Some(Instant::now());
                    let synced_to = chain.tip();
                    let balance = {
                        {
                            let synced_to_block = synced_to.block_id();
                            let task = graph.graph().canonicalization_task(
                                synced_to_block,
                                CanonicalizationParams::default(),
                            );
                            chain.canonicalize(task)
                        }
                        .balance(
                            graph.index.outpoints().iter().cloned(),
                            |(k, _), _| k == &Keychain::Internal,
                            0,
                        )
                    };
                    println!(
                        "[{:>10}s] synced to {} @ {} / {} | total: {}",
                        start.elapsed().as_secs_f32(),
                        synced_to.hash(),
                        synced_to.height(),
                        tip_height,
                        balance.total()
                    );
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
