use bdk_chain::{

    collections::BTreeMap,
    keychain_txout::KeychainTxOutIndex,
    local_chain::CheckPoint,
    tx_graph::TxGraph,

};
use bdk_core::spk_client::{FullScanRequest, SyncRequest};
use electrum_client::ElectrumApi;

use crate::BdkElectrumClient;

/// Configuration for the sync operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOptions {
    /// Whether to perform a fast sync (only revealed addresses) or full scan (lookahead until stop gap).
    ///
    /// - `true`: Sync only the scripts that are already revealed in the wallet.
    /// - `false`: Scan for new scripts until `stop_gap` of empty histories is found.
    pub fast: bool,

    /// The number of unused scripts to fetch before stopping (only used if `fast` is `false`).
    pub stop_gap: usize,

    /// The number of scripts to query in a single batch.
    pub batch_size: usize,

    /// Whether to fetch previous transaction outputs for fee calculation.
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
    /// Create options for a fast sync (revealed scripts only).
    pub fn fast() -> Self {
        Self {
            fast: true,
            ..Default::default()
        }
    }

    /// Create options for a full scan (unbounded discovery).
    pub fn full_scan() -> Self {
        Self {
            fast: false,
            ..Default::default()
        }
    }
}

/// A helper struct to sync a wallet (KeychainTxOutIndex) with an Electrum server.
///
/// This wrapper holds the [`BdkElectrumClient`] which maintains a cache of headers and transactions
/// to optimize repeated syncs.
pub struct ElectrumSync<'a, K, E> {
    wallet: &'a KeychainTxOutIndex<K>,
    client: BdkElectrumClient<E>,
}

impl<'a, K, E> ElectrumSync<'a, K, E>
where
    E: ElectrumApi,
    K: Ord + Clone + core::fmt::Debug + Send + Sync,
{
    /// Create a new `ElectrumSync` helper.
    ///
    /// # Arguments
    ///
    /// * `wallet` - The wallet index to sync.
    /// * `client` - The `BdkElectrumClient` to use for network requests.
    pub fn new(wallet: &'a KeychainTxOutIndex<K>, client: BdkElectrumClient<E>) -> Self {
        Self { wallet, client }
    }

    /// Access the underlying `BdkElectrumClient`.
    pub fn client(&self) -> &BdkElectrumClient<E> {
        &self.client
    }

    /// Perform the sync operation based on the provided `options`.
    ///
    /// Returns a tuple containing:
    /// - `Option<CheckPoint>`: The updated chain tip.
    /// - `TxGraph`: The graph of transactions found.
    /// - `Option<BTreeMap<K, u32>>`: The last active indices for each keychain (only for full scan).
    pub fn sync(
        &self,
        options: SyncOptions,
    ) -> Result<
        (
            Option<CheckPoint>,
            TxGraph<bdk_core::ConfirmationBlockTime>,
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

            // Note: sync() doesn't return last active indices in the same way full_scan does contextually,
            // but we can infer or simpler just return None for fast sync.
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
