use async_trait::async_trait;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, Script, Txid},
    collections::BTreeMap,
    keychain::LocalUpdate,
    local_chain::CheckPoint,
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
        prev_tip: Option<CheckPoint>,
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
        prev_tip: Option<CheckPoint>,
        misc_spks: impl IntoIterator<IntoIter = impl Iterator<Item = Script> + Send> + Send,
        txids: impl IntoIterator<IntoIter = impl Iterator<Item = Txid> + Send> + Send,
        outpoints: impl IntoIterator<IntoIter = impl Iterator<Item = OutPoint> + Send> + Send,
        parallel_requests: usize,
    ) -> Result<LocalUpdate<(), ConfirmationTimeAnchor>, Error> {
        self.scan(
            prev_tip,
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
        prev_tip: Option<CheckPoint>,
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

        let (new_blocks, mut last_cp) = 'retry: loop {
            let new_tip = loop {
                let hash = self.get_tip_hash().await?;
                let status = self.get_block_status(&hash).await?;
                if status.in_best_chain && status.next_best.is_none() {
                    break BlockId {
                        height: status.height.expect("must have height"),
                        hash,
                    };
                }
            };

            let mut new_blocks = core::iter::once((new_tip.height, new_tip.hash))
                .collect::<BTreeMap<u32, BlockHash>>();

            let mut agreement_cp = Option::<CheckPoint>::None;

            for cp in prev_tip.iter().flat_map(CheckPoint::iter) {
                let cp_block = cp.block_id();
                let hash = self.get_block_hash(cp_block.height).await?;
                if hash == cp_block.hash {
                    agreement_cp = Some(cp);
                    break;
                }
                new_blocks.insert(cp_block.height, hash);
            }

            // check for tip changes
            // retry if there are changes to the tip
            let status = self.get_block_status(&new_tip.hash).await?;

            if !status.in_best_chain || status.next_best.is_some() {
                continue 'retry;
            }

            // `new_blocks` should only include blocks that are actually new
            let new_blocks = match &agreement_cp {
                Some(agreement_cp) => new_blocks.split_off(&(agreement_cp.height() + 1)),
                None => new_blocks,
            };
            break 'retry (new_blocks, agreement_cp);
        };

        // construct checkpoints
        for (&height, &hash) in new_blocks.iter() {
            last_cp = Some(match last_cp {
                Some(last_cp) => last_cp
                    .extend(BlockId { height, hash })
                    .expect("must extend checkpoint"),
                None => CheckPoint::new(BlockId { height, hash }),
            });
        }

        let tip = last_cp.expect("must have atleast one checkpoint");
        let mut update = LocalUpdate::<K, ConfirmationTimeAnchor>::new(tip.clone());

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
                        let anchor = map_confirmation_time_anchor(&tx.status, &tip);

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
                    if let Some(anchor) = map_confirmation_time_anchor(&tx_status, &tip) {
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
                let anchor = map_confirmation_time_anchor(&status, &tip);

                let _ = update.graph.insert_tx(tx);
                if let Some(anchor) = anchor {
                    let _ = update.graph.insert_anchor(txid, anchor);
                }
            }
        }

        if tip.hash() != self.get_block_hash(tip.height()).await? {
            // A reorg occurred, so let's find out where all the txids we found are now in the chain
            let txids_found = update
                .graph
                .full_txs()
                .map(|tx_node| tx_node.txid)
                .collect::<Vec<_>>();
            let new_update = EsploraAsyncExt::scan_without_keychain(
                self,
                Some(tip),
                [],
                txids_found,
                [],
                parallel_requests,
            )
            .await?;
            update.tip = new_update.tip;
            update.graph = new_update.graph;
            // update.chain = EsploraAsyncExt::scan_without_keychain(
            //     self,
            //     local_chain,
            //     [],
            //     txids_found,
            //     [],
            //     parallel_requests,
            // )
            // .await?
            // .chain;
        }

        Ok(update)
    }
}
