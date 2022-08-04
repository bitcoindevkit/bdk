// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bdk::bitcoin::secp256k1::Secp256k1;
use bdk::bitcoin::Amount;
use bdk::bitcoin::Network;
use bdk::bitcoincore_rpc::RpcApi;

use bdk::blockchain::rpc::{Auth, RpcBlockchain, RpcConfig};
use bdk::blockchain::ConfigurableBlockchain;

use bdk::keys::bip39::{Language, Mnemonic, WordCount};
use bdk::keys::{DerivableKey, GeneratableKey, GeneratedKey};

use bdk::miniscript::miniscript::Segwitv0;

use bdk::sled;
use bdk::template::Bip84;
use bdk::wallet::{signer::SignOptions, wallet_name_from_descriptor, AddressIndex, SyncOptions};
use bdk::KeychainKind;
use bdk::Wallet;

use bdk::blockchain::Blockchain;

use electrsd;

use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;

/// This example demonstrates a typical way to create a wallet and work with bdk.
///
/// This example bdk wallet is connected to a bitcoin core rpc regtest node,
/// and will attempt to receive, create and broadcast transactions.
///
/// To start a bitcoind regtest node programmatically, this example uses
/// `electrsd` library, which is also a bdk dev-dependency.
///
/// But you can start your own bitcoind backend, and the rest of the example should work fine.

fn main() -> Result<(), Box<dyn Error>> {
    // -- Setting up background bitcoind process

    println!(">> Setting up bitcoind");

    // Start the bitcoind process
    let bitcoind_conf = electrsd::bitcoind::Conf::default();

    // electrsd will automatically download the bitcoin core binaries
    let bitcoind_exe =
        electrsd::bitcoind::downloaded_exe_path().expect("We should always have downloaded path");

    // Launch bitcoind and gather authentication access
    let bitcoind = electrsd::bitcoind::BitcoinD::with_conf(bitcoind_exe, &bitcoind_conf).unwrap();
    let bitcoind_auth = Auth::Cookie {
        file: bitcoind.params.cookie_file.clone(),
    };

    // Get a new core address
    let core_address = bitcoind.client.get_new_address(None, None)?;

    // Generate 101 blocks and use the above address as coinbase
    bitcoind.client.generate_to_address(101, &core_address)?;

    println!(">> bitcoind setup complete");
    println!(
        "Available coins in Core wallet : {}",
        bitcoind.client.get_balance(None, None)?
    );

    // -- Setting up the Wallet

    println!("\n>> Setting up BDK wallet");

    // Get a random private key
    let xprv = generate_random_ext_privkey()?;

    // Use the derived descriptors from the privatekey to
    // create unique wallet name.
    // This is a special utility function exposed via `bdk::wallet_name_from_descriptor()`
    let wallet_name = wallet_name_from_descriptor(
        Bip84(xprv.clone(), KeychainKind::External),
        Some(Bip84(xprv.clone(), KeychainKind::Internal)),
        Network::Regtest,
        &Secp256k1::new(),
    )?;

    // Create a database (using default sled type) to store wallet data
    let mut datadir = PathBuf::from_str("/tmp/")?;
    datadir.push(".bdk-example");
    let database = sled::open(datadir)?;
    let database = database.open_tree(wallet_name.clone())?;

    // Create a RPC configuration of the running bitcoind backend we created in last step
    // Note: If you are using custom regtest node, use the appropriate url and auth
    let rpc_config = RpcConfig {
        url: bitcoind.params.rpc_socket.to_string(),
        auth: bitcoind_auth,
        network: Network::Regtest,
        wallet_name,
        sync_params: None,
    };

    // Use the above configuration to create a RPC blockchain backend
    let blockchain = RpcBlockchain::from_config(&rpc_config)?;

    // Combine Database + Descriptor to create the final wallet
    let wallet = Wallet::new(
        Bip84(xprv.clone(), KeychainKind::External),
        Some(Bip84(xprv.clone(), KeychainKind::Internal)),
        Network::Regtest,
        database,
    )?;

    // The `wallet` and the `blockchain` are independent structs.
    // The wallet will be used to do all wallet level actions
    // The blockchain can be used to do all blockchain level actions.
    // For certain actions (like sync) the wallet will ask for a blockchain.

    // Sync the wallet
    // The first sync is important as this will instantiate the
    // wallet files.
    wallet.sync(&blockchain, SyncOptions::default())?;

    println!(">> BDK wallet setup complete.");
    println!(
        "Available initial coins in BDK wallet : {} sats",
        wallet.get_balance()?
    );

    // -- Wallet transaction demonstration

    println!("\n>> Sending coins: Core --> BDK, 10 BTC");
    // Get a new address to receive coins
    let bdk_new_addr = wallet.get_address(AddressIndex::New)?.address;

    // Send 10 BTC from core wallet to bdk wallet
    bitcoind.client.send_to_address(
        &bdk_new_addr,
        Amount::from_btc(10.0)?,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    // Confirm transaction by generating 1 block
    bitcoind.client.generate_to_address(1, &core_address)?;

    // Sync the BDK wallet
    // This time the sync will fetch the new transaction and update it in
    // wallet database
    wallet.sync(&blockchain, SyncOptions::default())?;

    println!(">> Received coins in BDK wallet");
    println!(
        "Available balance in BDK wallet: {} sats",
        wallet.get_balance()?
    );

    println!("\n>> Sending coins: BDK --> Core, 5 BTC");
    // Attempt to send back 5.0 BTC to core address by creating a transaction
    //
    // Transactions are created using a `TxBuilder`.
    // This helps us to systematically build a transaction with all
    // required customization.
    // A full list of APIs offered by `TxBuilder` can be found at
    // https://docs.rs/bdk/latest/bdk/wallet/tx_builder/struct.TxBuilder.html
    let mut tx_builder = wallet.build_tx();

    // For a regular transaction, just set the recipient and amount
    tx_builder.set_recipients(vec![(core_address.script_pubkey(), 500000000)]);

    // Finalize the transaction and extract the PSBT
    let (mut psbt, _) = tx_builder.finish()?;

    // Set signing option
    let signopt = SignOptions {
        assume_height: None,
        ..Default::default()
    };

    // Sign the psbt
    wallet.sign(&mut psbt, signopt)?;

    // Extract the signed transaction
    let tx = psbt.extract_tx();

    // Broadcast the transaction
    blockchain.broadcast(&tx)?;

    // Confirm transaction by generating some blocks
    bitcoind.client.generate_to_address(1, &core_address)?;

    // Sync the BDK wallet
    wallet.sync(&blockchain, SyncOptions::default())?;

    println!(">> Coins sent to Core wallet");
    println!(
        "Remaining BDK wallet balance: {} sats",
        wallet.get_balance()?
    );
    println!("\nCongrats!! you made your first test transaction with bdk and bitcoin core.");

    Ok(())
}

// Helper function demonstrating privatekey extraction using bip39 mnemonic
// The mnemonic can be shown to user to safekeeping and the same wallet
// private descriptors can be recreated from it.
fn generate_random_ext_privkey() -> Result<impl DerivableKey<Segwitv0> + Clone, Box<dyn Error>> {
    // a Bip39 passphrase can be set optionally
    let password = Some("random password".to_string());

    // Generate a random mnemonic, and use that to create a "DerivableKey"
    let mnemonic: GeneratedKey<_, _> = Mnemonic::generate((WordCount::Words12, Language::English))
        .map_err(|e| e.expect("Unknown Error"))?;

    // `Ok(mnemonic)` would also work if there's no passphrase and it would
    // yield the same result as this construct with `password` = `None`.
    Ok((mnemonic, password))
}
