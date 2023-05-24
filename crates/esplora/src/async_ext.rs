use async_trait::async_trait;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, Script, Txid},
    collections::BTreeMap,
    keychain::LocalUpdate,
    BlockId, ConfirmationTimeAnchor,
};
use esplora_client::{Error, OutputStatus, TxStatus};
use futures::{stream::FuturesOrdered, TryStreamExt};

use crate::map_confirmation_time_anchor;

/// Trait to extend [`esplora_client::AsyncClient`] functionality.
///
/// This is the async version of [`EsploraExt`]. Refer to
/// [crate-level documentation] for more.
///
/// [`EsploraExt`]: crate::EsploraExt
/// [crate-level documentation]: crate
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EsploraAsyncExt {
    /// Scan the blockchain (via esplora) for the data specified and returns a
    /// [`LocalUpdate<K, ConfirmationTimeAnchor>`].
    ///
    /// - `local_chain`: the most recent block hashes present locally
    /// - `keychain_spks`: keychains that we want to scan transactions for
    /// - `txids`: transactions for which we want updated [`ConfirmationTimeAnchor`]s
    /// - `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to included in the update
    ///
    /// The scan for each keychain stops after a gap of `stop_gap` script pubkeys with no associated
    /// transactions. `parallel_requests` specifies the max number of HTTP requests to make in
    /// parallel.
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
    ) -> Result<LocalUpdate<K, ConfirmationTimeAnchor>, Error>;

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
    ) -> Result<LocalUpdate<(), ConfirmationTimeAnchor>, Error> {
        self.scan(
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
        .await
    }
}

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
    ) -> Result<LocalUpdate<K, ConfirmationTimeAnchor>, Error> {
        let parallel_requests = Ord::max(parallel_requests, 1);

        let (mut update, tip_at_start) = loop {
            let mut update = LocalUpdate::<K, ConfirmationTimeAnchor>::default();

            for (&height, &original_hash) in local_chain.iter().rev() {
                let update_block_id = BlockId {
                    height,
                    hash: self.get_block_hash(height).await?,
                };
                let _ = update
                    .chain
                    .insert_block(update_block_id)
                    .expect("cannot repeat height here");
                if update_block_id.hash == original_hash {
                    break;
                }
            }

            let tip_at_start = BlockId {
                height: self.get_height().await?,
                hash: self.get_tip_hash().await?,
            };

            if update.chain.insert_block(tip_at_start).is_ok() {
                break (update, tip_at_start);
            }
        };

        for (keychain, spks) in keychain_spks {
            let mut spks = spks.into_iter();
            let mut last_active_index = None;
            let mut empty_scripts = 0;
            type IndexWithTxs = (u32, Vec<esplora_client::Tx>);

            loop {
                let futures = (0..parallel_requests)
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
                    .collect::<FuturesOrdered<_>>();

                let n_futures = futures.len();

                for (index, related_txs) in futures.try_collect::<Vec<IndexWithTxs>>().await? {
                    if related_txs.is_empty() {
                        empty_scripts += 1;
                    } else {
                        last_active_index = Some(index);
                        empty_scripts = 0;
                    }
                    for tx in related_txs {
                        let anchor = map_confirmation_time_anchor(&tx.status, tip_at_start);

                        let _ = update.graph.insert_tx(tx.to_tx());
                        if let Some(anchor) = anchor {
                            let _ = update.graph.insert_anchor(tx.txid, anchor);
                        }
                    }
                }

                if n_futures == 0 || empty_scripts >= stop_gap {
                    break;
                }
            }

            if let Some(last_active_index) = last_active_index {
                update.keychain.insert(keychain, last_active_index);
            }
        }

        for txid in txids.into_iter() {
            if update.graph.get_tx(txid).is_none() {
                match self.get_tx(&txid).await? {
                    Some(tx) => {
                        let _ = update.graph.insert_tx(tx);
                    }
                    None => continue,
                }
            }
            match self.get_tx_status(&txid).await? {
                tx_status if tx_status.confirmed => {
                    if let Some(anchor) = map_confirmation_time_anchor(&tx_status, tip_at_start) {
                        let _ = update.graph.insert_anchor(txid, anchor);
                    }
                }
                _ => continue,
            }
        }

        for op in outpoints.into_iter() {
            let mut op_txs = Vec::with_capacity(2);
            if let (
                Some(tx),
                tx_status @ TxStatus {
                    confirmed: true, ..
                },
            ) = (
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
                let txid = tx.txid();
                let anchor = map_confirmation_time_anchor(&status, tip_at_start);

                let _ = update.graph.insert_tx(tx);
                if let Some(anchor) = anchor {
                    let _ = update.graph.insert_anchor(txid, anchor);
                }
            }
        }

        if tip_at_start.hash != self.get_block_hash(tip_at_start.height).await? {
            // A reorg occurred, so let's find out where all the txids we found are now in the chain
            let txids_found = update
                .graph
                .full_txs()
                .map(|tx_node| tx_node.txid)
                .collect::<Vec<_>>();
            update.chain = EsploraAsyncExt::scan_without_keychain(
                self,
                local_chain,
                [],
                txids_found,
                [],
                parallel_requests,
            )
            .await?
            .chain;
        }

        Ok(update)
    }
}
