use std::str::FromStr;

use bitcoin::{
    Network,
    util::bip32::{self, ExtendedPrivKey},
};
use esplora_client::Builder;

use bdk::{
    blockchain::esplora::EsploraBlockchain,
    database::MemoryDatabase,
    KeychainKind,
    SyncOptions,
    template::Bip84, wallet::{AddressIndex, export::FullyNodedExport}, Wallet,
};
use bdk::blockchain::Blockchain;

use crate::utils::tor::{start_tor, use_tor};
use crate::utils::tx::build_signed_tx;

pub mod utils;

/// This will create a wallet from an xpriv and get the balance by connecting to an Esplora server,
/// using non blocking asynchronous calls with `reqwest`.
/// If enough amount is available, this will send a transaction to an address.
/// Otherwise, this will display a wallet address to receive funds.
///
/// This can be run with `cargo run --no-default-features --features="use-esplora-reqwest, reqwest-default-tls, async-interface" --example esplora_backend_asynchronous`
/// in the root folder.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let network = Network::Signet;

    let xpriv = "tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy";

    let esplora_url = "https://explorer.bc-2.jp/api";

    run(&network, esplora_url, xpriv).await;
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

async fn run(network: &Network, esplora_url: &str, xpriv: &str) {
    let xpriv = bip32::ExtendedPrivKey::from_str(xpriv).unwrap();

    let blockchain = if use_tor() {
        let tor_addrs = start_tor(None);
        let client = Builder::new(esplora_url)
            .proxy(format!("socks5://{}", tor_addrs.socks).as_ref())
            .build_async()
            .expect("Should never fail with no proxy and timeout");
        EsploraBlockchain::from_client(client, 20)
    } else {
        EsploraBlockchain::new(esplora_url, 20)
    };

    let wallet = create_wallet(network, &xpriv);

    wallet
        .sync(&blockchain, SyncOptions::default())
        .await
        .unwrap();

    let address = wallet.get_address(AddressIndex::New).unwrap().address;

    println!("address: {}", address);

    let balance = wallet.get_balance().unwrap();

    println!("Available coins in BDK wallet : {} sats", balance);

    if balance.confirmed > 10500 {
        // the wallet sends the amount to itself.
        let recipient_address = wallet
            .get_address(AddressIndex::New)
            .unwrap()
            .address
            .to_string();

        let amount = 9359;

        let tx = build_signed_tx(&wallet, &recipient_address, amount);

        let _ = blockchain.broadcast(&tx);

        println!("tx id: {}", tx.txid());
    } else {
        println!("Insufficient Funds. Fund the wallet with the address above");
    }

    let export = FullyNodedExport::export_wallet(&wallet, "exported wallet", true)
        .map_err(ToString::to_string)
        .map_err(bdk::Error::Generic)
        .unwrap();

    println!("------\nWallet Backup: {}", export.to_string());
}
