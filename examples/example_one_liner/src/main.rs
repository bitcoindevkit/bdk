use bdk_chain::{
    bitcoin::Network,
    keychain_txout::KeychainTxOutIndex,
    tx_graph::TxGraph,
    collections::BTreeMap,
    CheckPoint,
};
use bdk_core::{
    spk_client::{FullScanRequest, SyncRequest},
    ConfirmationBlockTime,
};
use bdk_electrum::{
    electrum_client::{self, ElectrumApi},
    BdkElectrumClient,
};

// -----------------------------------------------------------------------------
// ONE-LINER SYNC HELPER (Proposed API Pattern)
// -----------------------------------------------------------------------------

/// Configuration for the sync operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    pub fast: bool,
    pub stop_gap: usize,
    pub batch_size: usize,
    pub fetch_prev: bool,
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
    pub fn fast() -> Self {
        Self { fast: true, ..Default::default() }
    }
    pub fn full_scan() -> Self {
        Self { fast: false, ..Default::default() }
    }
}

pub struct ElectrumSync<'a, K, E> {
    wallet: &'a KeychainTxOutIndex<K>,
    client: BdkElectrumClient<E>,
}

impl<'a, K, E> ElectrumSync<'a, K, E>
where
    E: ElectrumApi,
    K: Ord + Clone + core::fmt::Debug + Send + Sync,
{
    pub fn new(wallet: &'a KeychainTxOutIndex<K>, client: BdkElectrumClient<E>) -> Self {
        Self { wallet, client }
    }

    pub fn sync(
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
    
    let electrum_client = electrum_client::Client::new(ELECTRUM_URL)?;
    let bdk_client = BdkElectrumClient::new(electrum_client);

    let syncer = ElectrumSync::new(&wallet_index, bdk_client);

    println!("Starting full scan...");
    let result = syncer.sync(SyncOptions::full_scan())?;
    println!("Sync finished! Found {} txs.", result.1.full_txs().count());

    println!("Starting fast sync...");
    let _ = syncer.sync(SyncOptions::fast())?;
    println!("Fast sync finished!");

    Ok(())
}
