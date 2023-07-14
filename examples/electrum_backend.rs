use std::str::FromStr;
use bitcoin::util::bip32;
use electrum_client::Client;

use jitash_bdk::bitcoin::util::bip32::ExtendedPrivKey;
use jitash_bdk::bitcoin::Network;
use jitash_bdk::blockchain::{Blockchain, ElectrumBlockchain};
use jitash_bdk::database::MemoryDatabase;
use jitash_bdk::template::Bip84;
use jitash_bdk::wallet::export::FullyNodedExport;
use jitash_bdk::{KeychainKind, SyncOptions, Wallet};
use jitash_bdk::wallet::AddressIndex;


pub mod utils;

use crate::utils::tx::build_signed_tx;

/// This will create a wallet from an xpriv and get the balance by connecting to an Electrum server.
/// If enough amount is available, this will send a transaction to an address.
/// Otherwise, this will display a wallet address to receive funds.
///
/// This can be run with `cargo run --example electrum_backend` in the root folder.
fn main() {
    let network = Network::Testnet;

    let xpriv = "tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy";

    let electrum_url = "ssl://electrum.blockstream.info:60002";

    run(&network, electrum_url, xpriv);
}

fn create_wallet(network: &Network, xpriv: &ExtendedPrivKey) -> Wallet<MemoryDatabase> {
    Wallet::new(
        Bip84(*xpriv, KeychainKind::External),
        Some(Bip84(*xpriv, KeychainKind::Internal)),
        *network,
        MemoryDatabase::default(),
    )
    .unwrap()
}

fn run(network: &Network, electrum_url: &str, xpriv: &str) {
    let xpriv = bip32::ExtendedPrivKey::from_str(xpriv).unwrap();

    // Apparently it works only with Electrs (not EletrumX)
    let blockchain = ElectrumBlockchain::from(Client::new(electrum_url).unwrap());

    let wallet = create_wallet(network, &xpriv);

    wallet.sync(&blockchain, SyncOptions::default()).unwrap();

    let address = wallet.get_address(AddressIndex::New).unwrap().address;

    println!("address: {}", address);

    let balance = wallet.get_balance().unwrap();

    println!("Available coins in BDK wallet : {} sats", balance);

    if balance.confirmed > 6500 {
        // the wallet sends the amount to itself.
        let recipient_address = wallet
            .get_address(AddressIndex::New)
            .unwrap()
            .address
            .to_string();

        let amount = 5359;

        let tx = build_signed_tx(&wallet, &recipient_address, amount);

        blockchain.broadcast(&tx).unwrap();

        println!("tx id: {}", tx.txid());
    } else {
        println!("Insufficient Funds. Fund the wallet with the address above");
    }

    let export = FullyNodedExport::export_wallet(&wallet, "exported wallet", true)
        .map_err(ToString::to_string)
        .map_err(jitash_bdk::Error::Generic)
        .unwrap();

    println!("------\nWallet Backup: {}", export.to_string());
}
