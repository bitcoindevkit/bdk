//! Simple one-liner Electrum sync helper
//!
//! This provides a clean API for synchronizing wallets with Electrum servers
//! while preserving cache and respecting BDK's architectural boundaries.

use bdk_chain::{keychain_txout::KeychainTxOutIndex, tx_graph::TxGraph};
use bdk_core::{
    collections::BTreeMap,
    spk_client::{FullScanRequest, SyncRequest},
    CheckPoint,
};
use bdk_electrum::BdkElectrumClient;
use electrum_client::Client;

/// Result type for Electrum synchronization
pub type ElectrumSyncResult<K> = (Option<CheckPoint>, TxGraph, Option<BTreeMap<K, u32>>);

/// Simple configuration for Electrum synchronization
#[derive(Debug, Clone, Copy)]
pub struct SyncOptions {
    pub fast: bool,
    pub stop_gap: usize,
    pub batch_size: usize,
    pub fetch_prev: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            fast: false,
            stop_gap: 25,
            batch_size: 30,
            fetch_prev: false,
        }
    }
}

impl SyncOptions {
    /// Create options for fast sync
    pub fn fast_sync() -> Self {
        Self {
            fast: true,
            ..Default::default()
        }
    }

    /// Create options for full scan
    pub fn full_scan() -> Self {
        Self::default()
    }

    pub fn with_stop_gap(mut self, stop_gap: usize) -> Self {
        self.stop_gap = stop_gap;
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    pub fn with_fetch_prev(mut self, fetch_prev: bool) -> Self {
        self.fetch_prev = fetch_prev;
        self
    }
}

/// Long-lived Electrum sync manager that preserves cache across operations
///
/// This struct holds a persistent connection to an Electrum server, maintaining
/// transaction and header caches between sync operations for better performance.
pub struct ElectrumSyncManager {
    client: BdkElectrumClient<Client>,
}

impl ElectrumSyncManager {
    /// Create a new sync manager with the given Electrum server URL
    pub fn new(url: &str) -> Result<Self, electrum_client::Error> {
        let client = Client::new(url)?;
        Ok(Self {
            client: BdkElectrumClient::new(client),
        })
    }

    /// One-liner synchronization - the main convenience method
    ///
    /// # Example
    /// ```text
    /// let manager = ElectrumSyncManager::new("tcp://electrum.example.com:50001")?;
    /// let result = manager.sync(&wallet, SyncOptions::full_scan())?;
    /// ```
    pub fn sync<K>(
        &self,
        wallet: &KeychainTxOutIndex<K>,
        options: SyncOptions,
    ) -> Result<ElectrumSyncResult<K>, electrum_client::Error>
    where
        K: Ord + Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        if options.fast {
            self.fast_sync(wallet, options.batch_size, options.fetch_prev)
        } else {
            self.full_scan(
                wallet,
                options.stop_gap,
                options.batch_size,
                options.fetch_prev,
            )
        }
    }

    /// One-liner fast sync (only checks revealed addresses)
    pub fn fast_sync<K>(
        &self,
        wallet: &KeychainTxOutIndex<K>,
        batch_size: usize,
        fetch_prev: bool,
    ) -> Result<ElectrumSyncResult<K>, electrum_client::Error>
    where
        K: Ord + Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        let request = Self::build_sync_request(wallet);
        let response = self.client.sync(request, batch_size, fetch_prev)?;

        Ok((response.chain_update, response.tx_update.into(), None))
    }

    /// One-liner full scan (scans until stop gap is reached)
    pub fn full_scan<K>(
        &self,
        wallet: &KeychainTxOutIndex<K>,
        stop_gap: usize,
        batch_size: usize,
        fetch_prev: bool,
    ) -> Result<ElectrumSyncResult<K>, electrum_client::Error>
    where
        K: Ord + Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        let request = Self::build_full_scan_request(wallet);
        let response = self
            .client
            .full_scan(request, stop_gap, batch_size, fetch_prev)?;

        Ok((
            response.chain_update,
            response.tx_update.into(),
            Some(response.last_active_indices),
        ))
    }

    /// Get a reference to the underlying client for advanced operations
    pub fn client(&self) -> &BdkElectrumClient<Client> {
        &self.client
    }

    /// Build a sync request based on revealed addresses.
    fn build_sync_request<K>(wallet: &KeychainTxOutIndex<K>) -> SyncRequest<(K, u32)>
    where
        K: Ord + Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        let mut builder = SyncRequest::builder();

        for keychain_id in wallet.keychains().map(|(k, _)| k) {
            for (index, spk) in wallet.revealed_keychain_spks(keychain_id.clone()) {
                builder = builder.spks_with_indexes([((keychain_id.clone(), index), spk)]);
            }
        }

        builder.build()
    }

    /// Build a full scan request using unbounded SPK iterators.
    fn build_full_scan_request<K>(wallet: &KeychainTxOutIndex<K>) -> FullScanRequest<K>
    where
        K: Ord + Clone + Send + Sync + std::fmt::Debug + 'static,
    {
        let mut builder = FullScanRequest::builder();

        // Add unbounded script iterators for each keychain
        for (keychain_id, spk_iter) in wallet.all_unbounded_spk_iters() {
            builder = builder.spks_for_keychain(keychain_id, spk_iter);
        }

        builder.build()
    }
}
