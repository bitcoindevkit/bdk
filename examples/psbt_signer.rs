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
use bdk::SyncOptions;
use bdk::{FeeRate, SignOptions, Wallet};
use bitcoin::{Address, Network};
use electrum_client::Client;
use std::error::Error;
use std::str::FromStr;

fn main() -> Result<(), Box<dyn Error>> {
    // test keys created with `bdk-cli key generate` and `bdk-cli key derive` commands
    let signing_external_descriptor = "wpkh([e9824965/84'/1'/0']tprv8fvem7qWxY3SGCQczQpRpqTKg455wf1zgixn6MZ4ze8gRfHjov5gXBQTadNfDgqs9ERbZZ3Bi1PNYrCCusFLucT39K525MWLpeURjHwUsfX/0/*)";
    let signing_internal_descriptor = "wpkh([e9824965/84'/1'/0']tprv8fvem7qWxY3SGCQczQpRpqTKg455wf1zgixn6MZ4ze8gRfHjov5gXBQTadNfDgqs9ERbZZ3Bi1PNYrCCusFLucT39K525MWLpeURjHwUsfX/1/*)";

    let watch_only_external_descriptor = "wpkh([e9824965/84'/1'/0']tpubDCcguXsm6uj79fSQt4V2EF7SF5b26zCuG2ZZNsbNQuw5G9YWSJuGhg2KknQBywRq4VGTu41zYTCh3QeVFyBdbsymgRX9Mrts94SW7obEdqs/0/*)";
    let watch_only_internal_descriptor = "wpkh([e9824965/84'/1'/0']tpubDCcguXsm6uj79fSQt4V2EF7SF5b26zCuG2ZZNsbNQuw5G9YWSJuGhg2KknQBywRq4VGTu41zYTCh3QeVFyBdbsymgRX9Mrts94SW7obEdqs/1/*)";

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
