use bdk::{
    bitcoin::{ Network, Address },
    chain::{indexed_tx_graph, local_chain, Append},
    wallet::{AddressIndex, WalletChangeSet},
    Wallet,
    SignOptions,
};
use bdk_bitcoind_rpc::{
    bitcoincore_rpc::{Auth, Client, RpcApi},
    EmittedUpdate, Emitter,
};
use bdk_file_store::Store;
use std::sync::mpsc::sync_channel;
use std::str::FromStr;

const DB_MAGIC: &str = "bdk-rpc-example";
const FALLBACK_HEIGHT: u32 = 2476300;
const CHANNEL_BOUND: usize = 100;
const SEND_AMOUNT: u64 = 5000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = std::env::temp_dir().join("bdk-rpc-example");
    let db = Store::<bdk::wallet::ChangeSet>::new_from_path(DB_MAGIC.as_bytes(), db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";

    let mut wallet = Wallet::new(
        external_descriptor,
        Some(internal_descriptor),
        db,
        Network::Testnet,
    )?;

    let address = wallet.get_address(AddressIndex::New);
    println!("Generated Address: {}", address);

    let balance = wallet.get_balance();
    println!("Wallet balance before syncing: {} sats", balance.total());


    print!("Syncing...");

    let client = Client::new(
        "127.0.0.1:18332",
        Auth::UserPass("bitcoin".to_string(), "password".to_string()),
    )?;
    
    println!("Connected to Bitcoin Core RPC at {:?}", client.get_blockchain_info().unwrap());

    wallet.mut_indexed_tx_graph().index.set_lookahead_for_all(20);

    let (chan, recv) = sync_channel::<(EmittedUpdate, u32)>(CHANNEL_BOUND);
    let prev_cp = wallet.latest_checkpoint();

    let join_handle = std::thread::spawn(move || -> anyhow::Result<()> {
        let mut tip_height = Option::<u32>::None;

        let mut emitter = Emitter::new(&client, FALLBACK_HEIGHT, prev_cp);
        loop {
            let item = emitter.emit_update()?;
            let is_mempool = item.is_mempool();

            if tip_height.is_none() || is_mempool {
                tip_height = Some(client.get_block_count()? as u32);
            }
            chan.send((item, tip_height.expect("must have tip height")))?;

            if !is_mempool {
                continue;
            } else {
                break;
            }
        }

        Ok(())
    });

    for (item, _) in recv {
        let chain_update = item.chain_update();

        let mut indexed_changeset = indexed_tx_graph::ChangeSet::default();

        let graph_update = item.indexed_tx_graph_update(bdk_bitcoind_rpc::confirmation_time_anchor);
        indexed_changeset.append(wallet.mut_indexed_tx_graph().insert_relevant_txs(graph_update));

        let chain_changeset = match chain_update {
            Some(update) => wallet.mut_local_chain().apply_update(update)?,
            None => local_chain::ChangeSet::default(),
        };

        let mut wallet_changeset = WalletChangeSet::from(chain_changeset);
        wallet_changeset.append(WalletChangeSet::from(indexed_changeset));
        wallet.stage(wallet_changeset);
        println!("scanning ...");
        wallet.commit()?;
    }

    let _ = join_handle.join().expect("failed to join chain source thread");

    let balance = wallet.get_balance();
    println!("Wallet balance after syncing: {} sats", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} sats to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let faucet_address = Address::from_str("tb1qw2c3lxufxqe2x9s4rdzh65tpf4d7fssjgh8nv6")?
        .require_network(Network::Testnet)?;

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(faucet_address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let (mut psbt, _) = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx();
    let client = Client::new(
        "127.0.0.1:18332",
        Auth::UserPass("bitcoin".to_string(), "password".to_string()),
    )?;
    client.send_raw_transaction(&tx)?;
    println!("Tx broadcasted! Txid: {}", tx.txid());

    Ok(())
}
