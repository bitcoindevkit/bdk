extern crate electrum_client;

use electrum_client::Client;

fn main() {
    let mut client = Client::new_ssl(
        "electrum2.hodlister.co:50002",
        Some("electrum2.hodlister.co"),
    )
    .unwrap();
    let res = client.server_features();
    println!("{:#?}", res);
}
