// Copyright (c) 2020-2022 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use bdk::blockchain::{Blockchain, ElectrumBlockchain};
use bdk::database::MemoryDatabase;
use bdk::wallet::AddressIndex;
use bdk::{descriptor, SyncOptions};
use bdk::{FeeRate, SignOptions, Wallet};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::{Address, Network};
use electrum_client::Client;
use miniscript::descriptor::DescriptorSecretKey;
use std::error::Error;
use std::str::FromStr;

/// This example shows how to sign and broadcast the transaction for a PSBT (Partially Signed
/// Bitcoin Transaction) for a single key, witness public key hash (WPKH) based descriptor wallet.
/// The electrum protocol is used to sync blockchain data from the testnet bitcoin network and
/// wallet data is stored in an ephemeral in-memory database. The process steps are:
/// 1. Create a "signing" wallet and a "watch-only" wallet based on the same private keys.
/// 2. Deposit testnet funds into the watch only wallet.
/// 3. Sync the watch only wallet and create a spending transaction to return all funds to the testnet faucet.
/// 4. Sync the signing wallet and sign and finalize the PSBT created by the watch only wallet.
/// 5. Broadcast the transactions from the finalized PSBT.
fn main() -> Result<(), Box<dyn Error>> {
    // test key created with `bdk-cli key generate` and `bdk-cli key derive` commands
    let external_secret_xkey = DescriptorSecretKey::from_str("[e9824965/84'/1'/0']tprv8fvem7qWxY3SGCQczQpRpqTKg455wf1zgixn6MZ4ze8gRfHjov5gXBQTadNfDgqs9ERbZZ3Bi1PNYrCCusFLucT39K525MWLpeURjHwUsfX/0/*").unwrap();
    let internal_secret_xkey = DescriptorSecretKey::from_str("[e9824965/84'/1'/0']tprv8fvem7qWxY3SGCQczQpRpqTKg455wf1zgixn6MZ4ze8gRfHjov5gXBQTadNfDgqs9ERbZZ3Bi1PNYrCCusFLucT39K525MWLpeURjHwUsfX/1/*").unwrap();

    let secp = Secp256k1::new();
    let external_public_xkey = external_secret_xkey.to_public(&secp).unwrap();
    let internal_public_xkey = internal_secret_xkey.to_public(&secp).unwrap();

    let signing_external_descriptor = descriptor!(wpkh(external_secret_xkey)).unwrap();
    let signing_internal_descriptor = descriptor!(wpkh(internal_secret_xkey)).unwrap();

    let watch_only_external_descriptor = descriptor!(wpkh(external_public_xkey)).unwrap();
    let watch_only_internal_descriptor = descriptor!(wpkh(internal_public_xkey)).unwrap();

    // create client for Blockstream's testnet electrum server
    let blockchain =
        ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?);

    // create watch only wallet
    let watch_only_wallet: Wallet<MemoryDatabase> = Wallet::new(
        watch_only_external_descriptor,
        Some(watch_only_internal_descriptor),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    // create signing wallet
    let signing_wallet: Wallet<MemoryDatabase> = Wallet::new(
        signing_external_descriptor,
        Some(signing_internal_descriptor),
        Network::Testnet,
        MemoryDatabase::default(),
    )?;

    println!("Syncing watch only wallet.");
    watch_only_wallet.sync(&blockchain, SyncOptions::default())?;

    // get deposit address
    let deposit_address = watch_only_wallet.get_address(AddressIndex::New)?;

    let balance = watch_only_wallet.get_balance()?;
    println!("Watch only wallet balances in SATs: {}", balance);

    if balance.get_total() < 10000 {
        println!(
            "Send at least 10000 SATs (0.0001 BTC) from the u01.net testnet faucet to address '{addr}'.\nFaucet URL: https://bitcoinfaucet.uo1.net/?to={addr}",
            addr = deposit_address.address
        );
    } else if balance.get_spendable() < 10000 {
        println!(
            "Wait for at least 10000 SATs of your wallet transactions to be confirmed...\nBe patient, this could take 10 mins or longer depending on how testnet is behaving."
        );
        for tx_details in watch_only_wallet
            .list_transactions(false)?
            .iter()
            .filter(|txd| txd.received > 0 && txd.confirmation_time.is_none())
        {
            println!(
                "See unconfirmed tx for {} SATs: https://mempool.space/testnet/tx/{}",
                tx_details.received, tx_details.txid
            );
        }
    } else {
        println!("Creating a PSBT sending 9800 SATs plus fee to the u01.net testnet faucet return address 'tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt'.");
        let return_address = Address::from_str("tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt")?;
        let mut builder = watch_only_wallet.build_tx();
        builder
            .add_recipient(return_address.script_pubkey(), 9_800)
            .enable_rbf()
            .fee_rate(FeeRate::from_sat_per_vb(1.0));

        let (mut psbt, details) = builder.finish()?;
        println!("Transaction details: {:#?}", details);
        println!("Unsigned PSBT: {}", psbt);

        // Sign and finalize the PSBT with the signing wallet
        let finalized = signing_wallet.sign(&mut psbt, SignOptions::default())?;
        assert!(finalized, "The PSBT was not finalized!");
        println!("The PSBT has been signed and finalized.");

        // Broadcast the transaction
        let raw_transaction = psbt.extract_tx();
        let txid = raw_transaction.txid();

        blockchain.broadcast(&raw_transaction)?;
        println!("Transaction broadcast! TXID: {txid}.\nExplorer URL: https://mempool.space/testnet/tx/{txid}", txid = txid);
    }

    Ok(())
}
