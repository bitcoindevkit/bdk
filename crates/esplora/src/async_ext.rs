use async_trait::async_trait;
use bdk_chain::{
    bitcoin::{BlockHash, OutPoint, Script, Txid},
    collections::BTreeMap,
    keychain::LocalUpdate,
    local_chain::CheckPoint,
    BlockId, ConfirmationTimeAnchor, TxGraph,
};
use esplora_client::{Error, OutputStatus, TxStatus};
use futures::{stream::FuturesOrdered, TryStreamExt};

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

        let (tip, _) = construct_update_tip(self, prev_tip).await?;
        let mut make_anchor = crate::confirmation_time_anchor_maker(&tip);
        let mut update = LocalUpdate::<K, ConfirmationTimeAnchor>::new(tip);

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
                        let anchor = make_anchor(&tx.status);

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
                    if let Some(anchor) = make_anchor(&tx_status) {
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
                let anchor = make_anchor(&status);

                let _ = update.graph.insert_tx(tx);
                if let Some(anchor) = anchor {
                    let _ = update.graph.insert_anchor(txid, anchor);
                }
            }
        }

        // If a reorg occured during the update, anchors may be wrong. We handle this by scrapping
        // all anchors, reconstructing checkpoints and reconstructing anchors.
        while self.get_block_hash(update.tip.height()).await? != update.tip.hash() {
            let (new_tip, _) = construct_update_tip(self, Some(update.tip.clone())).await?;
            make_anchor = crate::confirmation_time_anchor_maker(&new_tip);

            // Reconstruct graph with only transactions (no anchors).
            update.graph = TxGraph::new(update.graph.full_txs().map(|n| n.tx.clone()));
            update.tip = new_tip;

            // Re-fetch anchors.
            let anchors = {
                let mut a = Vec::new();
                for n in update.graph.full_txs() {
                    let status = self.get_tx_status(&n.txid).await?;
                    if !status.confirmed {
                        continue;
                    }
                    if let Some(anchor) = make_anchor(&status) {
                        a.push((n.txid, anchor));
                    }
                }
                a
            };
            for (txid, anchor) in anchors {
                let _ = update.graph.insert_anchor(txid, anchor);
            }
        }

        Ok(update)
    }
}

/// Constructs a new checkpoint tip that can "connect" to our previous checkpoint history. We return
/// the new checkpoint tip alongside the height of agreement between the two histories (if any).
#[allow(clippy::result_large_err)]
async fn construct_update_tip(
    client: &esplora_client::AsyncClient,
    prev_tip: Option<CheckPoint>,
) -> Result<(CheckPoint, Option<u32>), Error> {
    let new_tip_height = client.get_height().await?;

    // If esplora returns a tip height that is lower than our previous tip, then checkpoints do not
    // need updating. We just return the previous tip and use that as the point of agreement.
    if let Some(prev_tip) = prev_tip.as_ref() {
        if new_tip_height < prev_tip.height() {
            return Ok((prev_tip.clone(), Some(prev_tip.height())));
        }
    }

    // Grab latest blocks from esplora atomically first. We assume that deeper blocks cannot be
    // reorged. This ensures that our checkpoint history is consistent.
    let mut new_blocks = client
        .get_blocks(Some(new_tip_height))
        .await?
        .into_iter()
        .zip((0..new_tip_height).rev())
        .map(|(b, height)| (height, b.id))
        .collect::<BTreeMap<u32, BlockHash>>();

    let mut agreement_cp = Option::<CheckPoint>::None;

    for cp in prev_tip.iter().flat_map(CheckPoint::iter) {
        let cp_block = cp.block_id();

        // We check esplora blocks cached in `new_blocks` first, keeping the checkpoint history
        // consistent even during reorgs.
        let hash = match new_blocks.get(&cp_block.height) {
            Some(&hash) => hash,
            None => {
                assert!(
                    new_tip_height >= cp_block.height,
                    "already checked that esplora's tip cannot be smaller"
                );
                let hash = client.get_block_hash(cp_block.height).await?;
                new_blocks.insert(cp_block.height, hash);
                hash
            }
        };

        if hash == cp_block.hash {
            agreement_cp = Some(cp);
            break;
        }
    }

    let agreement_height = agreement_cp.as_ref().map(CheckPoint::height);

    let new_tip = new_blocks
        .into_iter()
        // Prune `new_blocks` to only include blocks that are actually new.
        .filter(|(height, _)| Some(*height) > agreement_height)
        .map(|(height, hash)| BlockId { height, hash })
        .fold(agreement_cp, |prev_cp, block| {
            Some(match prev_cp {
                Some(cp) => cp.extend(block).expect("must extend cp"),
                None => CheckPoint::new(block),
            })
        })
        .expect("must have at least one checkpoint");

    Ok((new_tip, agreement_height))
}
