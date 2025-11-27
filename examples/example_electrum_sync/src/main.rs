use bdk_chain::keychain_txout::KeychainTxOutIndex;
use example_electrum_sync::{ElectrumSyncManager, SyncOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing ElectrumSyncManager...");
    let wallet: KeychainTxOutIndex<()> = KeychainTxOutIndex::default();
    let manager = ElectrumSyncManager::new("ssl://electrum.blockstream.info:50001")?;

    // Fast sync
    let _ = manager.sync(&wallet, SyncOptions::fast_sync())?;

    // Full scan with custom stop gap
    let _ = manager.sync(&wallet, SyncOptions::full_scan().with_stop_gap(50))?;

    // Full scan with custom batch size
    let _ = manager.sync(&wallet, SyncOptions::full_scan().with_batch_size(100))?;

    Ok(())
}
