use bdk::bitcoin::{Address, Network};
use bdk::blockchain::{Blockchain, ElectrumBlockchain};
use bdk::database::MemoryDatabase;
use bdk::hwi::{types::HWIChain, HWIClient};
use bdk::signer::SignerOrdering;
use bdk::wallet::{hardwaresigner::HWISigner, AddressIndex};
use bdk::{FeeRate, KeychainKind, SignOptions, SyncOptions, Wallet};
use electrum_client::Client;
use std::str::FromStr;
use std::sync::Arc;

// This example shows how to sync a wallet, create a transaction, sign it
// and broadcast it using an external hardware wallet.
// The hardware wallet must be connected to the computer and unlocked before
// running the example. Also, the `hwi` python package should be installed
// and available in the environment.
//
// To avoid loss of funds, consider using an hardware wallet simulator:
// * Coldcard: https://github.com/Coldcard/firmware
// * Ledger: https://github.com/LedgerHQ/speculos
// * Trezor: https://docs.trezor.io/trezor-firmware/core/emulator/index.html
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hold tight, I'm connecting to your hardware wallet...");

    // Listing all the available hardware wallet devices...
    let devices = HWIClient::enumerate()?;
    let first_device = devices
        .first()
        .expect("No devices found. Either plug in a hardware wallet, or start a simulator.");
    // ...and creating a client out of the first one
    let client = HWIClient::get_client(first_device, true, HWIChain::Test)?;
    println!("Look what I found, a {}!", first_device.model);

    // Getting the HW's public descriptors
    let descriptors = client.get_descriptors(None)?;
    println!(
        "The hardware wallet's descriptor is: {}",
        descriptors.receive[0]
    );

    // Creating a custom signer from the device
    let custom_signer = HWISigner::from_device(first_device, HWIChain::Test)?;
    let mut wallet = Wallet::new(
        &descriptors.receive[0],
        Some(&descriptors.internal[0]),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    // Adding the hardware signer to the BDK wallet
    wallet.add_signer(
        KeychainKind::External,
        SignerOrdering(200),
        Arc::new(custom_signer),
    );

    // create client for Blockstream's testnet electrum server
    let blockchain =
        ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?);

    println!("Syncing the wallet...");
    wallet.sync(&blockchain, SyncOptions::default())?;

    // get deposit address
    let deposit_address = wallet.get_address(AddressIndex::New)?;

    let balance = wallet.get_balance()?;
    println!("Wallet balances in SATs: {}", balance);

    if balance.get_total() < 10000 {
        println!(
            "Send some sats from the u01.net testnet faucet to address '{addr}'.\nFaucet URL: https://bitcoinfaucet.uo1.net/?to={addr}",
            addr = deposit_address.address
        );
        return Ok(());
    }

    let return_address = Address::from_str("tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt")?;
    let (mut psbt, _details) = {
        let mut builder = wallet.build_tx();
        builder
            .drain_wallet()
            .drain_to(return_address.script_pubkey())
            .enable_rbf()
            .fee_rate(FeeRate::from_sat_per_vb(5.0));
        builder.finish()?
    };

    // `sign` will call the hardware wallet asking for a signature
    assert!(
        wallet.sign(&mut psbt, SignOptions::default())?,
        "The hardware wallet couldn't finalize the transaction :("
    );

    println!("Let's broadcast your tx...");
    let raw_transaction = psbt.extract_tx();
    let txid = raw_transaction.txid();

    blockchain.broadcast(&raw_transaction)?;
    println!("Transaction broadcasted! TXID: {txid}.\nExplorer URL: https://mempool.space/testnet/tx/{txid}", txid = txid);

    Ok(())
}
