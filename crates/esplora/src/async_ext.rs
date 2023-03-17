use std::collections::BTreeMap;

use async_trait::async_trait;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, Script, Txid},
    chain_graph::ChainGraph,
    keychain::KeychainScan,
    sparse_chain, BlockId, ConfirmationTime,
};
use esplora_client::{Error, OutputStatus};
use futures::stream::{FuturesOrdered, TryStreamExt};

use crate::map_confirmation_time;

#[cfg(feature = "async")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EsploraAsyncExt {
    /// Scan the blockchain (via esplora) for the data specified and returns a [`KeychainScan`].
    ///
    /// - `local_chain`: the most recent block hashes present locally
    /// - `keychain_spks`: keychains that we want to scan transactions for
    /// - `txids`: transactions for which we want updated [`ChainPosition`]s
    /// - `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to included in the update
    ///
    /// The scan for each keychain stops after a gap of `stop_gap` script pubkeys with no associated
    /// transactions. `parallel_requests` specifies the max number of HTTP requests to make in
    /// parallel.
    ///
    /// [`ChainPosition`]: bdk_chain::sparse_chain::ChainPosition
    #[allow(clippy::result_large_err)] // FIXME
    async fn scan<K: Ord + Clone + Send>(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        keychain_spks: BTreeMap<
            K,
            impl IntoIterator<IntoIter = impl Iterator<Item = (u32, Script)> + Send> + Send,
        >,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<KeychainScan<K, ConfirmationTime>, Error>;

    /// Convenience method to call [`scan`] without requiring a keychain.
    ///
    /// [`scan`]: EsploraAsyncExt::scan
    #[allow(clippy::result_large_err)] // FIXME
    async fn scan_without_keychain(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = Script> + Send> + Send,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        parallel_requests: usize,
    ) -> Result<ChainGraph<ConfirmationTime>, Error> {
        let wallet_scan = self
            .scan(
                local_chain,
                [(
                    (),
                    misc_spks
                        .into_iter()
                        .enumerate()
                        .map(|(i, spk)| (i as u32, spk)),
                )]
                .into(),
                txids,
                outpoints,
                usize::MAX,
                parallel_requests,
            )
            .await?;

        Ok(wallet_scan.update)
    }
}

#[cfg(feature = "async")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl EsploraAsyncExt for esplora_client::AsyncClient {
    #[allow(clippy::result_large_err)] // FIXME
    async fn scan<K: Ord + Clone + Send>(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        keychain_spks: BTreeMap<
            K,
            impl IntoIterator<IntoIter = impl Iterator<Item = (u32, Script)> + Send> + Send,
        >,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<KeychainScan<K, ConfirmationTime>, Error> {
        let txids = txids.into_iter();
        let outpoints = outpoints.into_iter();
        let parallel_requests = parallel_requests.max(1);
        let mut scan = KeychainScan::default();
        let update = &mut scan.update;
        let last_active_indices = &mut scan.last_active_indices;

        for (&height, &original_hash) in local_chain.iter().rev() {
            let update_block_id = BlockId {
                height,
                hash: self.get_block_hash(height).await?,
            };
            let _ = update
                .insert_checkpoint(update_block_id)
                .expect("cannot repeat height here");
            if update_block_id.hash == original_hash {
                break;
            }
        }
        let tip_at_start = BlockId {
            height: self.get_height().await?,
            hash: self.get_tip_hash().await?,
        };
        if let Err(failure) = update.insert_checkpoint(tip_at_start) {
            match failure {
                sparse_chain::InsertCheckpointError::HashNotMatching { .. } => {
                    // there was a re-org before we started scanning. We haven't consumed any iterators, so calling this function recursively is safe.
                    return EsploraAsyncExt::scan(
                        self,
                        local_chain,
                        keychain_spks,
                        txids,
                        outpoints,
                        stop_gap,
                        parallel_requests,
                    )
                    .await;
                }
            }
        }

        for (keychain, spks) in keychain_spks {
            let mut spks = spks.into_iter();
            let mut last_active_index = None;
            let mut empty_scripts = 0;
            type IndexWithTxs = (u32, Vec<esplora_client::Tx>);

            loop {
                let futures: FuturesOrdered<_> = (0..parallel_requests)
                    .filter_map(|_| {
                        let (index, script) = spks.next()?;
                        let client = self.clone();
                        Some(async move {
                            let mut related_txs = client.scripthash_txs(&script, None).await?;

                            let n_confirmed =
                                related_txs.iter().filter(|tx| tx.status.confirmed).count();
                            // esplora pages on 25 confirmed transactions. If there are 25 or more we
                            // keep requesting to see if there's more.
                            if n_confirmed >= 25 {
                                loop {
                                    let new_related_txs = client
                                        .scripthash_txs(
                                            &script,
                                            Some(related_txs.last().unwrap().txid),
                                        )
                                        .await?;
                                    let n = new_related_txs.len();
                                    related_txs.extend(new_related_txs);
                                    // we've reached the end
                                    if n < 25 {
                                        break;
                                    }
                                }
                            }

                            Result::<_, esplora_client::Error>::Ok((index, related_txs))
                        })
                    })
                    .collect();

                let n_futures = futures.len();

                let idx_with_tx: Vec<IndexWithTxs> = futures.try_collect().await?;

                for (index, related_txs) in idx_with_tx {
                    if related_txs.is_empty() {
                        empty_scripts += 1;
                    } else {
                        last_active_index = Some(index);
                        empty_scripts = 0;
                    }
                    for tx in related_txs {
                        let confirmation_time =
                            map_confirmation_time(&tx.status, tip_at_start.height);

                        if let Err(failure) = update.insert_tx(tx.to_tx(), confirmation_time) {
                            use bdk_chain::{
                                chain_graph::InsertTxError, sparse_chain::InsertTxError::*,
                            };
                            match failure {
                                InsertTxError::Chain(TxTooHigh { .. }) => {
                                    unreachable!("chain position already checked earlier")
                                }
                                InsertTxError::Chain(TxMovedUnexpectedly { .. })
                                | InsertTxError::UnresolvableConflict(_) => {
                                    /* implies reorg during a scan. We deal with that below */
                                }
                            }
                        }
                    }
                }

                if n_futures == 0 || empty_scripts >= stop_gap {
                    break;
                }
            }

            if let Some(last_active_index) = last_active_index {
                last_active_indices.insert(keychain, last_active_index);
            }
        }

        for txid in txids {
            let (tx, tx_status) =
                match (self.get_tx(&txid).await?, self.get_tx_status(&txid).await?) {
                    (Some(tx), Some(tx_status)) => (tx, tx_status),
                    _ => continue,
                };

            let confirmation_time = map_confirmation_time(&tx_status, tip_at_start.height);

            if let Err(failure) = update.insert_tx(tx, confirmation_time) {
                use bdk_chain::{chain_graph::InsertTxError, sparse_chain::InsertTxError::*};
                match failure {
                    InsertTxError::Chain(TxTooHigh { .. }) => {
                        unreachable!("chain position already checked earlier")
                    }
                    InsertTxError::Chain(TxMovedUnexpectedly { .. })
                    | InsertTxError::UnresolvableConflict(_) => {
                        /* implies reorg during a scan. We deal with that below */
                    }
                }
            }
        }

        for op in outpoints {
            let mut op_txs = Vec::with_capacity(2);
            if let (Some(tx), Some(tx_status)) = (
                self.get_tx(&op.txid).await?,
                self.get_tx_status(&op.txid).await?,
            ) {
                op_txs.push((tx, tx_status));
                if let Some(OutputStatus {
                    txid: Some(txid),
                    status: Some(spend_status),
                    ..
                }) = self.get_output_status(&op.txid, op.vout as _).await?
                {
                    if let Some(spend_tx) = self.get_tx(&txid).await? {
                        op_txs.push((spend_tx, spend_status));
                    }
                }
            }

            for (tx, status) in op_txs {
                let confirmation_time = map_confirmation_time(&status, tip_at_start.height);

                if let Err(failure) = update.insert_tx(tx, confirmation_time) {
                    use bdk_chain::{chain_graph::InsertTxError, sparse_chain::InsertTxError::*};
                    match failure {
                        InsertTxError::Chain(TxTooHigh { .. }) => {
                            unreachable!("chain position already checked earlier")
                        }
                        InsertTxError::Chain(TxMovedUnexpectedly { .. })
                        | InsertTxError::UnresolvableConflict(_) => {
                            /* implies reorg during a scan. We deal with that below */
                        }
                    }
                }
            }
        }

        let reorg_occurred = {
            if let Some(checkpoint) = update.chain().latest_checkpoint() {
                self.get_block_hash(checkpoint.height).await? != checkpoint.hash
            } else {
                false
            }
        };

        if reorg_occurred {
            // A reorg occurred, so let's find out where all the txids we found are in the chain now.
            // XXX: collect required because of weird type naming issues
            let txids_found = update
                .chain()
                .txids()
                .map(|(_, txid)| *txid)
                .collect::<Vec<_>>();
            scan.update = EsploraAsyncExt::scan_without_keychain(
                self,
                local_chain,
                [],
                txids_found,
                [],
                parallel_requests,
            )
            .await?;
        }

        Ok(scan)
    }
}
