use bdk::{
    blockchain::esplora::{esplora_client, BlockingClientExt},
    wallet::AddressIndex,
    Wallet,
};
use bdk_test_client::{RpcApi, TestClient};
use bitcoin::{Amount, Network};
use rand::Rng;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let _ = env_logger::init();
    const DESCRIPTOR: &'static str ="tr([73c5da0a/86'/0'/0']tprv8cSrHfiTQQWzKVejDHvBcvW4pdLEDLMvtVdbUXFfceQ4kbZKMsuFWbd3LUN3omNrQfafQaPwXUFXtcofkE9UjFZ3i9deezBHQTGvYV2xUzz/0/*)";
    const CHANGE_DESCRIPTOR: &'static str = "tr(tprv8ZgxMBicQKsPeQe98SGJ53vEJ7MNEFkQ4CkZmrr6PNom3vn6GqxuyoE78smkzpuP347zR9MXPg38PoZ8tbxLqSx4CufufHAGbQ9Hf7yTTwn/44'/0'/0'/1/*)#pxy2d75a";

    let mut test_client = TestClient::default();
    let esplora_url = format!(
        "http://{}",
        test_client.electrsd.esplora_url.as_ref().unwrap()
    );
    let client = esplora_client::Builder::new(&esplora_url).build_blocking()?;

    let wallet = Wallet::new(DESCRIPTOR, Some(CHANGE_DESCRIPTOR), Network::Regtest)
        .expect("parsing descriptors failed");
    // note we don't *need* the Mutex for this example but it helps to show when the wallet does and
    // doesn't need to be mutablek
    let wallet = std::sync::Mutex::new(wallet);
    let n_initial_transactions = 10;

    let addresses = {
        // we need it to be mutable to get a new address.
        // This incremenents the derivatoin index of the keychain.
        let mut wallet = wallet.lock().unwrap();
        core::iter::repeat_with(|| wallet.get_address(AddressIndex::New))
            .filter(|_| rand::thread_rng().gen_bool(0.5))
            .take(n_initial_transactions)
            .collect::<Vec<_>>()
    };

    // get some coins for the internal node
    test_client.generate(100, None);

    for address in addresses {
        let exp_txid = test_client
            .send_to_address(
                &address,
                Amount::from_sat(10_000),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("tx should send");
        eprintln!(
            "💸 sending some coins to: {} (index {}) in tx {}",
            address, address.index, exp_txid
        );
        // sometimes generate a block after we send coins to the address
        if rand::thread_rng().gen_bool(0.3) {
            let height = test_client.generate(1, None);
            eprintln!("📦 created a block at height {}", height);
        }
    }

    let wait_for_esplora_sync = std::time::Duration::from_secs(5);

    println!("⏳ waiting {}s for esplora to catch up..", wait_for_esplora_sync.as_secs());
    std::thread::sleep(wait_for_esplora_sync);


    let wallet_scan_input = {
        let wallet = wallet.lock().unwrap();
        wallet.start_wallet_scan()
    };

    let start = std::time::Instant::now();
    let stop_gap = 5;
    eprintln!(
        "🔎 startig scanning all keychains with stop gap of {}",
        stop_gap
    );
    let wallet_scan = client.wallet_scan(wallet_scan_input, stop_gap, &Default::default(), 5)?;

    // we've got an update so briefly take a lock the wallet to apply it
    {
        let mut wallet = wallet.lock().unwrap();
        match wallet.apply_wallet_scan(wallet_scan) {
            Ok(changes) => {
                eprintln!("🎉 success! ({}ms)", start.elapsed().as_millis());
                eprintln!("wallet balance after: {:?}", wallet.get_balance());
                //XXX: esplora is not indexing mempool transactions right now (or not doing it fast enough)
                eprintln!(
                    "wallet found {} new transactions",
                    changes.tx_additions().count(),
                );
                if changes.tx_additions().count() != n_initial_transactions {
                    eprintln!(
                        "(it should have found {} but maybe stop gap wasn't large enough?)",
                        n_initial_transactions
                    );
                }
            }
            Err(reason) => {
                eprintln!("❌ esplora produced invalid wallet scan {}", reason);
            }
        }
    }

    Ok(())
}
