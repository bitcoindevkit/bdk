use std::collections::{BTreeSet, HashSet};
use std::io::Write;

use bdk::bitcoin::Network;
use bdk::database::SqliteDatabase;
use bdk::wallet::{AddressIndex, Wallet};

use bdk_electrum::{electrum_client, BdkElectrumClient};
use bdk_wallet::rusqlite;

const EXTERNAL: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
const INTERNAL: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
const ELECTRUM_URL: &str = "ssl://electrum.blockstream.info:60002";
const NETWORK: Network = Network::Testnet;

const BDK_DB_PATH: &str = ".bdk-example.sqlite";
const BDK_WALLET_DB_PATH: &str = ".bdk-wallet-example.sqlite";

// Steps for migrating wallet parameters from the old `bdk` 0.30 to the new `bdk_wallet` 1.0.
// These steps can be applied to wallets backed by a SQLite database. For example to read an
// existing database, change `BDK_DB_PATH` above to point to the location of the old database
// file. You may also want to hard code the remaining parameters (descriptors, network, etc)
// to fit your setup. Note: because we're migrating to a new database there must not already
// exist a persisted wallet at the new path `BDK_WALLET_DB_PATH`.

// Usage: `cargo run --bin example_migrate_wallet`

fn main() -> anyhow::Result<()> {
    // Open old wallet
    let db = SqliteDatabase::new(BDK_DB_PATH);
    let old_wallet = Wallet::new(EXTERNAL, Some(INTERNAL), NETWORK, db)?;

    // Get last revealed addresses for each keychain
    let addr = old_wallet.get_address(AddressIndex::LastUnused)?;
    println!("Last revealed external {} {}", addr.index, addr.address);
    let external_derivation_index = addr.index;
    let last_revealed_external = addr.address.to_string();

    let addr = old_wallet.get_internal_address(AddressIndex::LastUnused)?;
    println!("Last revealed internal {} {}", addr.index, addr.address);
    let internal_derivation_index = addr.index;
    let last_revealed_internal = addr.address.to_string();

    // Get unspent balance
    let old_unspent = old_wallet.list_unspent()?;
    let old_balance = old_wallet.get_balance()?;
    println!("Balance before sync: {} sat", old_balance.get_total());

    // Create new wallet
    // For the new bdk wallet we pass in the same descriptors as before. If the given descriptors
    // contain secret keys the wallet will be able to sign transactions as well.
    let mut db = rusqlite::Connection::open(BDK_WALLET_DB_PATH)?;
    let mut new_wallet = match bdk_wallet::Wallet::create(EXTERNAL, INTERNAL)
        .network(bdk_wallet::bitcoin::Network::Signet)
        .create_wallet(&mut db)
    {
        Ok(wallet) => wallet,
        Err(_) => anyhow::bail!("should not have existing db"),
    };

    // Retore revealed addresses
    let _ = new_wallet.reveal_addresses_to(
        bdk_wallet::KeychainKind::External,
        external_derivation_index,
    );
    let _ = new_wallet.reveal_addresses_to(
        bdk_wallet::KeychainKind::Internal,
        internal_derivation_index,
    );

    // Remember to persist the new wallet
    new_wallet.persist(&mut db)?;

    println!("\n========== New database created. ==========");

    let addr = new_wallet
        .list_unused_addresses(bdk_wallet::KeychainKind::External)
        .last()
        .unwrap();
    assert_eq!(addr.to_string(), last_revealed_external);
    println!("Last revealed external {} {}", addr.index, addr.address);
    let addr = new_wallet
        .list_unused_addresses(bdk_wallet::KeychainKind::Internal)
        .last()
        .unwrap();
    println!("Last revealed internal {} {}", addr.index, addr.address);
    assert_eq!(addr.to_string(), last_revealed_internal);

    // Now that we migrated the wallet details, you likely want to rescan the blockchain
    // to restore the wallet's transaction data. Here we're using the bdk_electrum client
    // with a gap limit of 10. We also pass the `true` parameter to `full_scan` to indicate
    // we want to collect additional transaction data (previous txouts) that are important
    // for a fully functioning wallet.

    let client = BdkElectrumClient::new(electrum_client::Client::new(ELECTRUM_URL)?);
    let request = new_wallet.start_full_scan().inspect({
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

    let update = client.full_scan(request, 10, 5, true)?;

    new_wallet.apply_update(update)?;
    new_wallet.persist(&mut db)?;

    // mapping unspent outpoints to string
    let new_unspent: BTreeSet<_> = new_wallet
        .list_unspent()
        .map(|output| output.outpoint.to_string())
        .collect();
    assert_eq!(
        new_unspent,
        old_unspent
            .into_iter()
            .map(|output| output.outpoint.to_string())
            .collect::<BTreeSet<_>>()
    );

    let new_balance = new_wallet.balance();
    assert_eq!(new_balance.total().to_sat(), old_balance.get_total());
    println!("\nBalance after sync: {} sat", new_balance.total().to_sat());

    Ok(())
}
