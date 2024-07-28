use std::path::PathBuf;
use std::sync::mpsc::sync_channel;
use std::thread::spawn;
use std::time::Instant;

use clap::{self, Parser};

use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client, RpcApi},
    Emitter,
};
use bdk_wallet::{
    bitcoin::{Block, Network, Transaction},
    rusqlite::Connection,
    Wallet,
};

/// Bitcoind RPC example using `bdk_wallet::Wallet`.
///
/// This syncs the chain block-by-block and prints the current balance, transaction count and UTXO
/// count.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
pub struct Args {
    /// Wallet descriptor
    #[clap(env = "DESCRIPTOR")]
    pub descriptor: String,
    /// Wallet change descriptor
    #[clap(env = "CHANGE_DESCRIPTOR")]
    pub change_descriptor: String,
    /// Earliest block height to start sync from
    #[clap(env = "START_HEIGHT", long, default_value = "100000")]
    pub start_height: u32,
    /// Bitcoin network to connect to
    #[clap(env = "BITCOIN_NETWORK", long, default_value = "signet")]
    pub network: Network,

    /// RPC URL
    #[clap(env = "RPC_URL", long, default_value = "127.0.0.1:38332")]
    pub url: String,
    /// RPC auth cookie file
    #[clap(env = "RPC_COOKIE", long)]
    pub rpc_cookie: Option<PathBuf>,
    /// RPC auth username
    #[clap(env = "RPC_USER", long)]
    pub rpc_user: Option<String>,
    /// RPC auth password
    #[clap(env = "RPC_PASS", long)]
    pub rpc_pass: Option<String>,
}

impl Args {
    fn client(&self) -> anyhow::Result<Client> {
        Ok(Client::new(
            &self.url,
            match (&self.rpc_cookie, &self.rpc_user, &self.rpc_pass) {
                (None, None, None) => Auth::None,
                (Some(path), _, _) => Auth::CookieFile(path.clone()),
                (_, Some(user), Some(pass)) => Auth::UserPass(user.clone(), pass.clone()),
                (_, Some(_), None) => panic!("rpc auth: missing rpc_pass"),
                (_, None, Some(_)) => panic!("rpc auth: missing rpc_user"),
            },
        )?)
    }
}

#[derive(Debug)]
enum Emission {
    SigTerm,
    Block(bdk_bitcoind_rpc::BlockEvent<Block>),
    Mempool(Vec<(Transaction, u64)>),
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let rpc_client = args.client()?;
    println!(
        "Connected to Bitcoin Core RPC at {:?}",
        rpc_client.get_blockchain_info().unwrap()
    );

    let start_load_wallet = Instant::now();
    let mut conn = Connection::open_in_memory().expect("must open connection");

    let wallet_opt = Wallet::load()
        .descriptors(args.descriptor.clone(), args.change_descriptor.clone())
        .network(args.network)
        .load_wallet(&mut conn)?;
    let mut wallet = match wallet_opt {
        Some(wallet) => wallet,
        None => Wallet::create(args.descriptor, args.change_descriptor)
            .network(args.network)
            .create_wallet(&mut conn)?,
    };
    println!(
        "Loaded wallet in {}s",
        start_load_wallet.elapsed().as_secs_f32()
    );

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {}", balance.total());

    let wallet_tip = wallet.latest_checkpoint();
    println!(
        "Wallet tip: {} at height {}",
        wallet_tip.hash(),
        wallet_tip.height()
    );

    let (sender, receiver) = sync_channel::<Emission>(21);

    let signal_sender = sender.clone();
    ctrlc::set_handler(move || {
        signal_sender
            .send(Emission::SigTerm)
            .expect("failed to send sigterm")
    });

    let emitter_tip = wallet_tip.clone();
    spawn(move || -> Result<(), anyhow::Error> {
        let mut emitter = Emitter::new(&rpc_client, emitter_tip, args.start_height);
        while let Some(emission) = emitter.next_block()? {
            sender.send(Emission::Block(emission))?;
        }
        sender.send(Emission::Mempool(emitter.mempool()?))?;
        Ok(())
    });

    let mut blocks_received = 0_usize;
    for emission in receiver {
        match emission {
            Emission::SigTerm => {
                println!("Sigterm received, exiting...");
                break;
            }
            Emission::Block(block_emission) => {
                blocks_received += 1;
                let height = block_emission.block_height();
                let hash = block_emission.block_hash();
                let connected_to = block_emission.connected_to();
                let start_apply_block = Instant::now();
                wallet.apply_block_connected_to(&block_emission.block, height, connected_to)?;
                wallet.persist(&mut conn)?;
                let elapsed = start_apply_block.elapsed().as_secs_f32();
                println!(
                    "Applied block {} at height {} in {}s",
                    hash, height, elapsed
                );
            }
            Emission::Mempool(mempool_emission) => {
                let start_apply_mempool = Instant::now();
                wallet.apply_unconfirmed_txs(mempool_emission.iter().map(|(tx, time)| (tx, *time)));
                wallet.persist(&mut conn)?;
                println!(
                    "Applied unconfirmed transactions in {}s",
                    start_apply_mempool.elapsed().as_secs_f32()
                );
                break;
            }
        }
    }
    let wallet_tip_end = wallet.latest_checkpoint();
    let balance = wallet.balance();
    println!(
        "Synced {} blocks in {}s",
        blocks_received,
        start_load_wallet.elapsed().as_secs_f32(),
    );
    println!(
        "Wallet tip is '{}:{}'",
        wallet_tip_end.height(),
        wallet_tip_end.hash()
    );
    println!("Wallet balance is {}", balance.total());
    println!(
        "Wallet has {} transactions and {} utxos",
        wallet.transactions().count(),
        wallet.list_unspent().count()
    );

    Ok(())
}
