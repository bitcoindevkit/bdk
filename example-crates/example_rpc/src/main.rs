use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::sync_channel,
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime},
};

use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client, RpcApi},
    EmittedUpdate, Emitter,
};
use bdk_chain::{
    bitcoin::{address, Address, Transaction},
    indexed_tx_graph, keychain,
    local_chain::{self, LocalChain},
    Append, BlockId, ConfirmationTimeAnchor, IndexedTxGraph,
};
use example_cli::{
    anyhow,
    clap::{self, Args, Subcommand},
    CoinSelectionAlgo, Keychain,
};

const DB_MAGIC: &[u8] = b"bdk_example_rpc";
const DB_PATH: &str = ".bdk_example_rpc.db";
const CHANNEL_BOUND: usize = 10;
const LIVE_POLL_DUR_SECS: Duration = Duration::from_secs(15);

type ChangeSet = (
    local_chain::ChangeSet,
    indexed_tx_graph::ChangeSet<ConfirmationTimeAnchor, keychain::ChangeSet<Keychain>>,
);

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

#[derive(Subcommand, Debug, Clone)]
enum RpcCommands {
    /// Scans blocks via RPC (starting from last point of agreement) and stores/indexes relevant
    /// transactions
    Scan {
        /// Starting block height to fallback to if no point of agreement if found
        #[clap(env = "FALLBACK_HEIGHT", long, default_value = "0")]
        fallback_height: u32,
        /// The unused-scripts lookahead will be kept at this size
        #[clap(long, default_value = "10")]
        lookahead: u32,
        /// Whether to be live!
        #[clap(long, default_value = "false")]
        live: bool,
        #[clap(flatten)]
        rpc_args: RpcArgs,
    },
    /// Create and broadcast a transaction.
    Tx {
        value: u64,
        address: Address<address::NetworkUnchecked>,
        #[clap(short, default_value = "bnb")]
        coin_select: CoinSelectionAlgo,
        #[clap(flatten)]
        rpc_args: RpcArgs,
    },
}

impl RpcCommands {
    fn rpc_args(&self) -> &RpcArgs {
        match self {
            RpcCommands::Scan { rpc_args, .. } => rpc_args,
            RpcCommands::Tx { rpc_args, .. } => rpc_args,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let sigterm_flag = start_ctrlc_handler();

    let (args, keymap, index, db, init_changeset) =
        example_cli::init::<RpcCommands, ChangeSet>(DB_MAGIC, DB_PATH)?;

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
                |_| Err(anyhow::anyhow!("use `tx` instead")),
                general_cmd,
            );
            db.lock().unwrap().commit()?;
            return res;
        }
    };

    let rpc_client = {
        let a = rpc_cmd.rpc_args();
        Client::new(
            &a.url,
            match (&a.rpc_cookie, &a.rpc_user, &a.rpc_password) {
                (None, None, None) => Auth::None,
                (Some(path), _, _) => Auth::CookieFile(path.clone()),
                (_, Some(user), Some(pass)) => Auth::UserPass(user.clone(), pass.clone()),
                (_, Some(_), None) => panic!("rpc auth: missing rpc_pass"),
                (_, None, Some(_)) => panic!("rpc auth: missing rpc_user"),
            },
        )?
    };

    match rpc_cmd {
        RpcCommands::Scan {
            fallback_height,
            lookahead,
            live,
            ..
        } => {
            graph.lock().unwrap().index.set_lookahead_for_all(lookahead);

            let (chan, recv) = sync_channel::<(EmittedUpdate, u32)>(CHANNEL_BOUND);
            let prev_cp = chain.lock().unwrap().tip();

            let join_handle = std::thread::spawn(move || -> anyhow::Result<()> {
                let mut tip_height = Option::<u32>::None;

                let mut emitter = Emitter::new(&rpc_client, fallback_height, prev_cp);
                loop {
                    let item = emitter.emit_update()?;
                    let is_mempool = item.is_mempool();

                    if tip_height.is_none() || is_mempool {
                        tip_height = Some(rpc_client.get_block_count()? as u32);
                    }
                    chan.send((item, tip_height.expect("must have tip height")))?;

                    if !is_mempool {
                        // break if sigterm is detected
                        if sigterm_flag.load(Ordering::Acquire) {
                            break;
                        }
                        continue;
                    }

                    // everything after this point is a mempool update
                    // mempool update is emitted after we reach the chain tip
                    // if we are are in "sync-once" mode, we break here
                    // otherwise, we sleep or wait for sigterm
                    if !live || await_flag(&sigterm_flag, LIVE_POLL_DUR_SECS) {
                        break;
                    }
                }

                Ok(())
            });

            let mut start = Instant::now();

            for (item, tip_height) in recv {
                let is_mempool = item.is_mempool();
                let tip = item.checkpoint();
                let current_height = tip.height();

                let db_changeset = {
                    let mut indexed_changeset = indexed_tx_graph::ChangeSet::default();
                    let mut chain = chain.lock().unwrap();
                    let mut graph = graph.lock().unwrap();

                    let graph_update = {
                        let tx_filter = bdk_bitcoind_rpc::indexer_filter(
                            &mut graph.index,
                            &mut indexed_changeset.indexer,
                        );
                        let anchor_map = bdk_bitcoind_rpc::confirmation_time_anchor;
                        item.into_tx_graph_update(tx_filter, anchor_map)
                    };
                    indexed_changeset.append(graph.apply_update(graph_update));

                    let chain_changeset = chain.apply_update(local_chain::Update {
                        tip,
                        introduce_older_blocks: false,
                    })?;

                    (chain_changeset, indexed_changeset)
                };

                let mut db = db.lock().unwrap();
                db.stage(db_changeset);

                // print stuff every 3 seconds
                if start.elapsed() >= Duration::from_secs(3) {
                    start = Instant::now();
                    let balance = {
                        let chain = chain.lock().unwrap();
                        let graph = graph.lock().unwrap();
                        graph.graph().balance(
                            &*chain,
                            chain.tip().map_or(BlockId::default(), |cp| cp.block_id()),
                            graph.index.outpoints().iter().cloned(),
                            |(k, _), _| k == &Keychain::Internal,
                        )
                    };
                    println!(
                        "* scanned_to: {} / {} tip | total: {} sats",
                        if is_mempool {
                            "mempool".to_string()
                        } else {
                            current_height.to_string()
                        },
                        tip_height,
                        balance.confirmed
                            + balance.immature
                            + balance.trusted_pending
                            + balance.untrusted_pending
                    );
                }
            }

            db.lock().unwrap().commit()?;
            println!("commited to database!");

            join_handle
                .join()
                .expect("failed to join chain source thread")
        }
        RpcCommands::Tx {
            value,
            address,
            coin_select,
            ..
        } => {
            let chain = chain.lock().unwrap();
            let broadcast = move |tx: &Transaction| -> anyhow::Result<()> {
                rpc_client.send_raw_transaction(tx)?;
                Ok(())
            };
            example_cli::run_send_cmd(
                &graph,
                &db,
                &*chain,
                &keymap,
                coin_select,
                address
                    .require_network(args.network)
                    .expect("address has the wrong network"),
                value,
                broadcast,
            )
        }
    }
}

fn start_ctrlc_handler() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let cloned_flag = flag.clone();

    ctrlc::set_handler(move || cloned_flag.store(true, Ordering::Release));

    flag
}

fn await_flag(flag: &AtomicBool, duration: Duration) -> bool {
    let start = SystemTime::now();
    loop {
        if flag.load(Ordering::Acquire) {
            return true;
        }
        if SystemTime::now()
            .duration_since(start)
            .expect("should succeed")
            >= duration
        {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
