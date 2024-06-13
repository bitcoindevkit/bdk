use std::{collections::BTreeSet, io::Write, str::FromStr};

use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_wallet::{
    bitcoin::{Address, Amount, Network, Script},
    KeychainKind, SignOptions, Wallet,
};

use bdk_sqlite::{rusqlite::Connection, Store};
use bdk_wallet::chain::persist::PersistBackend;

const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 50;
const PARALLEL_REQUESTS: usize = 5;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let db_path = "bdk-esplora-async-example.sqlite";
    let conn = Connection::open(db_path)?;
    let mut db = Store::new(conn)?;
    let external_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    let internal_descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
    let changeset = db.load_changes()?;

    let mut wallet = Wallet::new_or_load(
        external_descriptor,
        internal_descriptor,
        changeset,
        Network::Signet,
    )?;

    let address = wallet.next_unused_address(KeychainKind::External);
    wallet.commit_to(&mut db)?;
    println!("Generated Address: {}", address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {} sats", balance.total());

    print!("Syncing...");
    let client = esplora_client::Builder::new("http://signet.bitcoindevkit.net").build_async()?;

    fn generate_inspect(kind: KeychainKind) -> impl FnMut(u32, &Script) + Send + Sync + 'static {
        let mut once = Some(());
        let mut stdout = std::io::stdout();
        move |spk_i, _| {
            match once.take() {
                Some(_) => print!("\nScanning keychain [{:?}]", kind),
                None => print!(" {:<3}", spk_i),
            };
            stdout.flush().expect("must flush");
        }
    }
    let request = wallet
        .start_full_scan()
        .inspect_spks_for_all_keychains({
            let mut once = BTreeSet::<KeychainKind>::new();
            move |keychain, spk_i, _| {
                match once.insert(keychain) {
                    true => print!("\nScanning keychain [{:?}]", keychain),
                    false => print!(" {:<3}", spk_i),
                }
                std::io::stdout().flush().expect("must flush")
            }
        })
        .inspect_spks_for_keychain(
            KeychainKind::External,
            generate_inspect(KeychainKind::External),
        )
        .inspect_spks_for_keychain(
            KeychainKind::Internal,
            generate_inspect(KeychainKind::Internal),
        );

    let mut update = client
        .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
        .await?;
    let now = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs();
    let _ = update.graph_update.update_last_seen_unconfirmed(now);

    wallet.apply_update(update)?;
    wallet.commit_to(&mut db)?;
    println!();

    let balance = wallet.balance();
    println!("Wallet balance after syncing: {} sats", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} sats to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let faucet_address = Address::from_str("mkHS9ne12qx9pS9VojpwU5xtRd4T7X7ZUt")?
        .require_network(Network::Signet)?;

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(faucet_address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let mut psbt = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx()?;
    client.broadcast(&tx).await?;
    println!("Tx broadcasted! Txid: {}", tx.compute_txid());

    Ok(())
}
