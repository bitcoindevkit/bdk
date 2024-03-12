use bdk::{
    bitcoin::{Block, Network, Transaction},
    wallet::Wallet,
};
use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client, RpcApi},
    Emitter,
};
use bdk_file_store::Store;
use clap::{self, Parser};
use std::{path::PathBuf, sync::mpsc::sync_channel, thread::spawn, time::Instant};

const DB_MAGIC: &str = "bdk-rpc-wallet-example";

/// Bitcoind RPC example using `bdk::Wallet`.
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
    pub change_descriptor: Option<String>,
    /// Earliest block height to start sync from
    #[clap(env = "START_HEIGHT", long, default_value = "481824")]
    pub start_height: u32,
    /// Bitcoin network to connect to
    #[clap(env = "BITCOIN_NETWORK", long, default_value = "testnet")]
    pub network: Network,
    /// Where to store wallet data
    #[clap(
        env = "BDK_DB_PATH",
        long,
        default_value = ".bdk_wallet_rpc_example.db"
    )]
    pub db_path: PathBuf,

    /// RPC URL
    #[clap(env = "RPC_URL", long, default_value = "127.0.0.1:8332")]
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
    let mut wallet = Wallet::new_or_load(
        &args.descriptor,
        args.change_descriptor.as_ref(),
        Store::<bdk::wallet::ChangeSet>::open_or_create_new(DB_MAGIC.as_bytes(), args.db_path)?,
        args.network,
    )?;
    println!(
        "Loaded wallet in {}s",
        start_load_wallet.elapsed().as_secs_f32()
    );

    let balance = wallet.get_balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

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

    spawn(move || -> Result<(), anyhow::Error> {
        let mut emitter = Emitter::new(&rpc_client, wallet_tip, args.start_height);
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
                wallet.commit()?;
                let elapsed = start_apply_block.elapsed().as_secs_f32();
                println!(
                    "Applied block {} at height {} in {}s",
                    hash, height, elapsed
                );
            }
            Emission::Mempool(mempool_emission) => {
                let start_apply_mempool = Instant::now();
                wallet.apply_unconfirmed_txs(mempool_emission.iter().map(|(tx, time)| (tx, *time)));
                wallet.commit()?;
                println!(
                    "Applied unconfirmed transactions in {}s",
                    start_apply_mempool.elapsed().as_secs_f32()
                );
                break;
            }
        }
    }
    let wallet_tip_end = wallet.latest_checkpoint();
    let balance = wallet.get_balance();
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
    println!("Wallet balance is {} sats", balance.total());
    println!(
        "Wallet has {} transactions and {} utxos",
        wallet.transactions().count(),
        wallet.list_unspent().count()
    );

    Ok(())
}
