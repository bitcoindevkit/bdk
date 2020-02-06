extern crate electrum_client;

use electrum_client::Client;

fn main() {
    // NOTE: This assumes Tor is running localy, with an unauthenticated Socks5 listening at
    // localhost:9050

    let mut client = Client::new_proxy("ozahtqwp25chjdjd.onion:50001", "127.0.0.1:9050").unwrap();
    let res = client.server_features();
    println!("{:#?}", res);

    // works both with onion v2/v3 (if your Tor supports them)
    let mut client = Client::new_proxy(
        "v7gtzf7nua6hdmb2wtqaqioqmesdb4xrlly4zwr7bvayxv2bpg665pqd.onion:50001",
        "127.0.0.1:9050",
    )
    .unwrap();
    let res = client.server_features();
    println!("{:#?}", res);
}
