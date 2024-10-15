use std::collections::HashSet;
use std::io::Write;

use bdk::bitcoin::Network;
use bdk::sled;
use bdk::wallet::{AddressIndex, Wallet};

use bdk_electrum::{electrum_client, BdkElectrumClient};
use bdk_wallet::rusqlite;

const DESC: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/0/*)";
const CHANGE_DESC: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/1/*)";
const ELECTRUM_URL: &str = "ssl://mempool.space:60602";
const NETWORK: Network = Network::Signet;

// Steps for migrating wallet details from bdk v0.29 to bdk_wallet v1.0. For this example we assume
// the previous wallet was backed by `sled` and the new wallet will use sqlite. (steps will be
// similar if coming from sqlite, but we can't easily depend on multiple versions of rusqlite
// in the same project).

// Run this with `cargo run` and optionally providing a path and tree name
// for an existing sled db

// Usage: `cargo run -- [db_path tree_name]`

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cargo_dir = std::env::var("CARGO_MANIFEST_DIR")?;

    // Accept a db config passed in via command line, or else create a new one for testing
    let mut args = std::env::args();
    _ = args.next();
    let args: Vec<String> = args.collect();

    let db = if args.len() < 2 {
        let path = format!("{}/sled", cargo_dir);
        sled::open(path)?.open_tree("wallet")?
    } else {
        let path = args[0].to_string();
        let tree_name = args[1].to_string();
        sled::open(path)?.open_tree(tree_name)?
    };

    // Open wallet
    let wallet = Wallet::new(DESC, Some(CHANGE_DESC), NETWORK, db)?;

    // Get last revealed addresses for each keychain
    let addr = wallet.get_address(AddressIndex::LastUnused)?;
    println!("Last revealed external {} {}", addr.index, addr.address);
    let last_revealed_external = addr.index;

    let addr = wallet.get_internal_address(AddressIndex::LastUnused)?;
    println!("Last revealed internal {} {}", addr.index, addr.address);
    let last_revealed_internal = addr.index;

    // Get descriptors
    // Or we can use the same `DESC` and `CHANGE_DESC` from above.
    // let descriptor = wallet.public_descriptor(KeychainKind::External)?.unwrap().to_string();
    // let change_descriptor = wallet.public_descriptor(KeychainKind::Internal)?.unwrap().to_string();

    // Note:
    // If wallet 1 was created with signing keys we could try to get the signers from it as well, however
    // we'll get a compiler error if we try to pass these to wallet 2, as it is using a different version
    // of rust-miniscript. Since the old bdk made you provide the descriptors at startup, we can assume
    // they are also available when doing the migration, so private keys will carry over if desired.
    // let signers_external = wallet.get_signers(KeychainKind::External).as_key_map(wallet.secp_ctx());
    // let signers_internal = wallet.get_signers(KeychainKind::Internal).as_key_map(wallet.secp_ctx());
    drop(wallet);

    // Create new wallet
    let new_db_path = format!("{}/wallet.sqlite", cargo_dir);
    let mut db = rusqlite::Connection::open(new_db_path)?;
    let mut wallet = match bdk_wallet::Wallet::load().load_wallet(&mut db)? {
        Some(wallet) => wallet,
        None => bdk_wallet::Wallet::create(DESC, CHANGE_DESC)
            .network(bdk_wallet::bitcoin::Network::Signet)
            .create_wallet(&mut db)?,
    };

    // Retore revealed addresses
    let _ = wallet.reveal_addresses_to(bdk_wallet::KeychainKind::External, last_revealed_external);
    let _ = wallet.reveal_addresses_to(bdk_wallet::KeychainKind::Internal, last_revealed_internal);

    // Remember to persist the new wallet
    wallet.persist(&mut db)?;

    println!("\n========== New database created. ==========");

    let addr = wallet
        .list_unused_addresses(bdk_wallet::KeychainKind::External)
        .last()
        .unwrap();
    println!("Last revealed external {} {}", addr.index, addr.address);
    let addr = wallet
        .list_unused_addresses(bdk_wallet::KeychainKind::Internal)
        .last()
        .unwrap();
    println!("Last revealed internal {} {}", addr.index, addr.address);

    // Now that we migrated the wallet details, you likely want to rescan the blockchain
    // to restore transaction data for wallet 2. Here we're using the bdk_electrum client
    // with a gap limit of 20. We also pass the `true` parameter to `full_scan` to indicate
    // we want to collect additional transaction data (previous txouts) that are important
    // for a fully functioning wallet.

    let client = BdkElectrumClient::new(electrum_client::Client::new(ELECTRUM_URL)?);
    let req = wallet.start_full_scan().inspect({
        let mut stdout = std::io::stdout();
        let mut once = HashSet::new();
        move |k, spk_i, _| {
            if once.insert(k) {
                print!("\nScanning keychain [{:?}]", k);
            }
            print!(" {:<3}", spk_i);
            stdout.flush().unwrap();
        }
    });

    let update = client.full_scan(req, 20, 1, true)?;

    wallet.apply_update(update)?;
    wallet.persist(&mut db)?;
    println!("\n{:#?}", wallet.balance());

    Ok(())
}
