use std::io::Write;

use anyhow::Ok;
use bdk_esplora::{esplora_client, EsploraAsyncExt};
use bdk_wallet::{
    bitcoin::{Amount, Network, Script},
    rusqlite::Connection,
    KeychainKind, SignOptions, Wallet,
};

const SEND_AMOUNT: Amount = Amount::from_sat(5000);
const STOP_GAP: usize = 20;
const PARALLEL_REQUESTS: usize = 3;

const NETWORK: Network = Network::Signet;
const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
const INTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
const ESPLORA_URL: &str = "http://signet.bitcoindevkit.net";

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let mut conn = Connection::open_in_memory().expect("must open connection");

    let wallet_opt = Wallet::load()
        .descriptors(EXTERNAL_DESC, INTERNAL_DESC)
        .network(NETWORK)
        .load_wallet(&mut conn)?;
    let mut wallet = match wallet_opt {
        Some(wallet) => wallet,
        None => Wallet::create(EXTERNAL_DESC, INTERNAL_DESC)
            .network(NETWORK)
            .create_wallet(&mut conn)?,
    };

    let address = wallet.next_unused_address(KeychainKind::External);
    wallet.persist(&mut conn)?;
    println!("Next unused address: ({}) {}", address.index, address);

    let balance = wallet.balance();
    println!("Wallet balance before syncing: {}", balance.total());

    print!("Syncing...");
    let client = esplora_client::Builder::new(ESPLORA_URL).build_async()?;

    fn generate_inspect(kind: KeychainKind) -> impl FnMut(u32, &Script) + Send + Sync + 'static {
        let mut once = Some(());
        let mut stdout = std::io::stdout();
        move |spk_i, _| {
            match once.take() {
                Some(_) => print!("\nScanning keychain [{:?}] {:<3}", kind, spk_i),
                None => print!(" {:<3}", spk_i),
            };
            stdout.flush().expect("must flush");
        }
    }
    let request = wallet
        .start_full_scan()
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

    println!();

    wallet.apply_update(update)?;
    wallet.persist(&mut conn)?;
    println!();

    let balance = wallet.balance();
    println!("Wallet balance after syncing: {}", balance.total());

    if balance.total() < SEND_AMOUNT {
        println!(
            "Please send at least {} to the receiving address",
            SEND_AMOUNT
        );
        std::process::exit(0);
    }

    let mut tx_builder = wallet.build_tx();
    tx_builder
        .add_recipient(address.script_pubkey(), SEND_AMOUNT)
        .enable_rbf();

    let mut psbt = tx_builder.finish()?;
    let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    assert!(finalized);

    let tx = psbt.extract_tx()?;
    client.broadcast(&tx).await?;
    println!("Tx broadcasted! Txid: {}", tx.compute_txid());

    Ok(())
}
