//! Example of a one-liner wallet sync using ElectrumSync.
//!
//! This example demonstrates how an application can build a "one-liner" Electrum sync
//! by composing existing BDK APIs.
//!
//! No new API is introduced in bdk_electrum.
//!
//! This example demonstrates how to:
//! 1. Create a wallet (KeychainTxOutIndex).
//! 2. Create an Electrum client.
//! 3. Use `ElectrumSync` for a "one-liner" sync.
//!
//! Note: This example requires an actual Electrum server URL to run successfully.
//! By default it tries to connect to a public testnet server.

use bdk_chain::{
    keychain_txout::KeychainTxOutIndex,
    tx_graph::TxGraph,
    collections::BTreeMap,
    CheckPoint,
};
use bdk_core::{
    spk_client::{FullScanRequest, SyncRequest},
    ConfirmationBlockTime,
};
use bdk_electrum::BdkElectrumClient;
use electrum_client::{self, ElectrumApi};

// -----------------------------------------------------------------------------
// ONE-LINER SYNC HELPER (Proposed API Pattern)
// -----------------------------------------------------------------------------

// NOTE: This helper is intentionally defined in the example.
// It demonstrates how an application may compose existing BDK APIs.
// This is NOT part of the bdk_electrum public API.

/// Configuration for the sync operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncOptions {
    fast: bool,
    stop_gap: usize,
    batch_size: usize,
    fetch_prev: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            fast: true,
            stop_gap: 20,
            batch_size: 10,
            fetch_prev: false,
        }
    }
}

impl SyncOptions {
    fn fast() -> Self {
        Self { fast: true, ..Default::default() }
    }
    
    fn full_scan() -> Self {
        Self { fast: false, ..Default::default() }
    }
}

struct ElectrumSync<'a, K, E> {
    wallet: &'a KeychainTxOutIndex<K>,
    client: BdkElectrumClient<E>,
}

impl<'a, K, E> ElectrumSync<'a, K, E>
where
    E: ElectrumApi,
    K: Ord + Clone + core::fmt::Debug + Send + Sync,
{
    fn new(wallet: &'a KeychainTxOutIndex<K>, client: BdkElectrumClient<E>) -> Self {
        Self { wallet, client }
    }

    fn sync(
        &self,
        options: SyncOptions,
    ) -> Result<
        (
            Option<CheckPoint>,
            TxGraph<ConfirmationBlockTime>,
            Option<BTreeMap<K, u32>>,
        ),
        electrum_client::Error,
    > {
        if options.fast {
            let request = SyncRequest::builder()
                .spks_with_indexes(
                    self.wallet
                        .revealed_spks(..)
                        .map(|(k, spk)| (k.1, spk.into())),
                )
                .build();

            let response = self
                .client
                .sync(request, options.batch_size, options.fetch_prev)?;

            Ok((
                response.chain_update,
                response.tx_update.into(),
                None,
            ))
        } else {
            let mut builder = FullScanRequest::builder();
            
            for (keychain, spks) in self.wallet.all_unbounded_spk_iters() {
                builder = builder.spks_for_keychain(keychain, spks);
            }

            let request = builder.build();

            let response = self.client.full_scan(
                request,
                options.stop_gap,
                options.batch_size,
                options.fetch_prev,
            )?;

            Ok((
                response.chain_update,
                response.tx_update.into(),
                Some(response.last_active_indices),
            ))
        }
    }
}

// -----------------------------------------------------------------------------
// EXAMPLE USAGE
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MyKeychain {
    External,
    Internal,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const ELECTRUM_URL: &str = "ssl://electrum.blockstream.info:60002"; // Testnet

    let mut wallet_index = KeychainTxOutIndex::<MyKeychain>::new(20, true);
    println!("Wallet index initialized.");

    // This descriptor is specific to Testnet.
    // In a real example we might parse it, but for now we just initialize index.
    
    // 2. Setup Electrum Client
    let electrum_client = electrum_client::Client::new(ELECTRUM_URL)?;
    // Wrap it in BdkElectrumClient (preserves cache)
    let bdk_client = BdkElectrumClient::new(electrum_client);

    // 3. One-Liner Sync
    // We create the helper.
    let syncer = ElectrumSync::new(&wallet_index, bdk_client);

    println!("Starting full scan...");
    
    // Perform a full scan (discovers scripts)
    let result = syncer.sync(SyncOptions::full_scan())?;
    
    // Ideally we would apply the result to the wallet here.
    // wallet_index.apply_update(result.1, result.2); // Conceptual, depends on specific Wallet/Index API for application.
    
    println!("Sync finished!");
    println!("New tip: {:?}", result.0);
    println!("Found transactions: {}", result.1.full_txs().count());

    // 4. Repeated Sync (Fast)
    // Suppose we just want to check revealed addresses for new txs (faster).
    println!("Starting fast sync...");
    let _fast_result = syncer.sync(SyncOptions::fast())?;
    
    println!("Fast sync finished!");

    Ok(())
}
