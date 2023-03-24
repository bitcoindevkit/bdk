use std::str::FromStr;

use bdk::{
    bitcoin::{consensus::deserialize, Address, Network, Transaction},
    KeychainKind, SignOptions, Wallet,
};

use bdk_rpc_wallet::{
    bitcoincore_rpc::{Auth, RpcApi},
    into_confirmation_time, into_keychain_scan, missing_full_txs, RpcClient, RpcWalletExt,
};

use bdk_file_store::KeychainStore;

const SEND_AMOUNT: u64 = 5000;

/// This is an demonstration example of using the [`bdk::Wallet`]  and syncing it with [`bdk_rpc_wallet::RpcClient`].
///
/// This example shows the basic workflow wallet creation. If the descriptors don't have enough balance to send
/// transaction, the code will terminate with a message  "Please send at least 5000 sats to the receiving address".
///
/// The example work with a local signet node with RPC and Wallet enabled. If you need to change the node credential
/// update the `client` construction segment below with your own details (RPC Port, username:password)
///
/// Note: Currently the RPC syncing works with legacy wallet, with BDB support. Which is a non-default core configuration.
/// Follow the below instruction to compile bitcoind with BDB support.
/// https://github.com/bitcoin/bitcoin/blob/master/doc/build-unix.md#berkeley-db
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hello, world!");

    let db_path = std::env::temp_dir().join("bdk-electrum-example");
    let db = KeychainStore::new_from_path(db_path)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/1/*)";

    let mut wallet = Wallet::new(
        external_descriptor,
        Some(internal_descriptor),
        db,
        Network::Testnet,
    )?;

    let address = wallet.get_address(bdk::wallet::AddressIndex::New);
    println!("Generated Address: {}", address);

    let balance = wallet.get_balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client = {
        let rpc_url = "127.0.0.1:38332".to_string();
        // Change the auth below to match your node config before running the example
        let rpc_auth = Auth::UserPass("mempool".to_string(), "mempool".to_string());
        RpcClient::new("wallet-name".to_string(), rpc_url, rpc_auth)?
    };

    let local_chain = wallet.checkpoints();

    let response_idx_txheight =
        client.scan(local_chain, wallet.dump_spks().map(|(_, spk)| spk.clone()))?;

    let new_txids = missing_full_txs(&response_idx_txheight, &wallet);

    let new_txs = new_txids
        .iter()
        .map(|txid| {
            let tx_data = client.get_transaction(&txid, Some(true))?.hex;
            let tx: Transaction = deserialize(&tx_data)?;
            Ok(tx)
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

    let response_idx_conftime = into_confirmation_time(&client, response_idx_txheight)?;
    println!();
    let keychain_scan =
        into_keychain_scan::<KeychainKind, _, _>(response_idx_conftime, new_txs, &wallet)?;
    wallet.apply_update(keychain_scan)?;
    wallet.commit()?;

    let balance = wallet.get_balance();
    println!("Wallet balance after syncing: {} sats", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} sats to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let faucet_address = Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?;

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(faucet_address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let (mut psbt, _) = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx();
    client.send_raw_transaction(&tx)?;
    println!("Tx broadcasted! Txid: {}", tx.txid());

    Ok(())
}
