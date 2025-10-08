use crate::BdkElectrumClient;
use bdk_chain::{keychain_txout::KeychainTxOutIndex, tx_graph::TxGraph};
use bdk_core::{
    collections::BTreeMap,
    spk_client::{FullScanRequest, SyncRequest},
    CheckPoint,
};
use electrum_client::Client;

/// The result of an Electrum sync operation.
///
/// Contains:
/// - `chain_update`: Optional checkpoint update for the blockchain state
/// - `tx_update`: Transaction graph updates (new transactions, anchors, etc.)
/// - `keychain_update`: Optional updates to keychain indices (only in full scan mode)
pub type ElectrumSyncResult<K> = (Option<CheckPoint>, TxGraph, Option<BTreeMap<K, u32>>);

/// A builder for synchronizing a wallet with an Electrum server.
///
/// This struct provides a fluent interface for configuring and executing
/// wallet synchronization with an Electrum server. It supports both fast
/// sync (checking only revealed addresses) and full scan (scanning until
/// stop gap is reached) modes.
///
/// # Type Parameters
///
/// * `K` - The keychain identifier type, must be `Ord + Clone + Send + Sync + Debug`
pub struct ElectrumSync<'a, K> {
    wallet: &'a mut KeychainTxOutIndex<K>,
    url: Option<String>,
    stop_gap: usize,
    batch_size: usize,
    fast: bool,
    fetch_prev: bool,
}

impl<'a, K> ElectrumSync<'a, K>
where
    K: Ord + Clone + Send + Sync + core::fmt::Debug + 'static,
{
    /// Create a new `ElectrumSync` builder for the given wallet.
    pub fn new(wallet: &'a mut KeychainTxOutIndex<K>) -> Self {
        Self {
            wallet,
            url: None,
            stop_gap: 25,
            batch_size: 30,
            fast: false,
            fetch_prev: false,
        }
    }

    /// Set the Electrum server URL.
    ///
    /// If not set, defaults to `"tcp://electrum.blockstream.info:50001"`
    pub fn url(mut self, url: &str) -> Self {
        self.url = Some(url.to_string());
        self
    }

    /// Set the stop gap for full scans.
    pub fn stop_gap(mut self, sg: usize) -> Self {
        self.stop_gap = sg;
        self
    }

    /// Set the batch size for Electrum requests.
    ///
    /// This controls how many script pubkeys are requested in a single batch call to the
    /// Electrum server. Defaults to 30.
    pub fn batch_size(mut self, bs: usize) -> Self {
        self.batch_size = bs;
        self
    }

    /// Enable fast sync mode.
    ///
    /// Fast sync only checks already revealed script pubkeys, while full scan (the default)
    /// scans all possible addresses until the stop gap is reached.
    pub fn fast_sync(mut self) -> Self {
        self.fast = true;
        self
    }

    /// Enable fetching previous transaction outputs for fee calculation.
    ///
    /// calculating fees on transactions where your wallet doesn't own the inputs.
    pub fn fetch_prev_txouts(mut self) -> Self {
        self.fetch_prev = true;
        self
    }

    /// Execute the sync request and return the updates.
    pub fn request(self) -> Result<ElectrumSyncResult<K>, electrum_client::Error> {
        let url = self
            .url
            .as_deref()
            .unwrap_or("tcp://electrum.blockstream.info:50001");

        let electrum = Client::new(url)?;
        let bdk_client = BdkElectrumClient::new(electrum);

        if self.fast {
            let sync_request = self.build_sync_request();
            let sync_response = bdk_client.sync(sync_request, self.batch_size, self.fetch_prev)?;

            Ok((
                sync_response.chain_update,
                sync_response.tx_update.into(),
                None,
            ))
        } else {
            let full_scan_request = self.build_full_scan_request();
            let full_scan_response = bdk_client.full_scan(
                full_scan_request,
                self.stop_gap,
                self.batch_size,
                self.fetch_prev,
            )?;

            Ok((
                full_scan_response.chain_update,
                full_scan_response.tx_update.into(),
                Some(full_scan_response.last_active_indices),
            ))
        }
    }

    fn build_sync_request(&self) -> SyncRequest<(K, u32)> {
        let mut builder = SyncRequest::builder();

        // Add revealed scripts from the wallet
        for keychain_id in self.wallet.keychains().map(|(k, _)| k) {
            for (index, spk) in self.wallet.revealed_keychain_spks(keychain_id.clone()) {
                builder = builder.spks_with_indexes([((keychain_id.clone(), index), spk)]);
            }
        }

        builder.build()
    }

    fn build_full_scan_request(&self) -> FullScanRequest<K> {
        let mut builder = FullScanRequest::builder();

        // Add unbounded script iterators for each keychain
        for (keychain_id, spk_iter) in self.wallet.all_unbounded_spk_iters() {
            builder = builder.spks_for_keychain(keychain_id, spk_iter);
        }
        builder.build()
    }
}
