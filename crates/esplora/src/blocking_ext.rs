use bdk_chain::bitcoin::{BlockHash, OutPoint, Script, Txid};
use bdk_chain::collections::BTreeMap;
use bdk_chain::local_chain::CheckPoint;
use bdk_chain::{keychain::LocalUpdate, ConfirmationTimeAnchor};
use bdk_chain::{BlockId, TxGraph};
use esplora_client::{Error, OutputStatus, TxStatus};

/// Trait to extend [`esplora_client::BlockingClient`] functionality.
///
/// Refer to [crate-level documentation] for more.
///
/// [crate-level documentation]: crate
pub trait EsploraExt {
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
    fn scan<K: Ord + Clone>(
        &self,
        prev_tip: Option<CheckPoint>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<LocalUpdate<K, ConfirmationTimeAnchor>, Error>;

    /// Convenience method to call [`scan`] without requiring a keychain.
    ///
    /// [`scan`]: EsploraExt::scan
    #[allow(clippy::result_large_err)] // FIXME
    fn scan_without_keychain(
        &self,
        prev_tip: Option<CheckPoint>,
        misc_spks: impl IntoIterator<Item = Script>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
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
    }
}

impl EsploraExt for esplora_client::BlockingClient {
    fn scan<K: Ord + Clone>(
        &self,
        prev_tip: Option<CheckPoint>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<LocalUpdate<K, ConfirmationTimeAnchor>, Error> {
        let parallel_requests = Ord::max(parallel_requests, 1);

        let (tip, _) = construct_update_tip(self, prev_tip)?;
        let mut make_anchor = crate::confirmation_time_anchor_maker(&tip);
        let mut update = LocalUpdate::<K, ConfirmationTimeAnchor>::new(tip);

        for (keychain, spks) in keychain_spks {
            let mut spks = spks.into_iter();
            let mut last_active_index = None;
            let mut empty_scripts = 0;
            type IndexWithTxs = (u32, Vec<esplora_client::Tx>);

            loop {
                let handles = (0..parallel_requests)
                    .filter_map(
                        |_| -> Option<std::thread::JoinHandle<Result<IndexWithTxs, _>>> {
                            let (index, script) = spks.next()?;
                            let client = self.clone();
                            Some(std::thread::spawn(move || {
                                let mut related_txs = client.scripthash_txs(&script, None)?;

                                let n_confirmed =
                                    related_txs.iter().filter(|tx| tx.status.confirmed).count();
                                // esplora pages on 25 confirmed transactions. If there are 25 or more we
                                // keep requesting to see if there's more.
                                if n_confirmed >= 25 {
                                    loop {
                                        let new_related_txs = client.scripthash_txs(
                                            &script,
                                            Some(related_txs.last().unwrap().txid),
                                        )?;
                                        let n = new_related_txs.len();
                                        related_txs.extend(new_related_txs);
                                        // we've reached the end
                                        if n < 25 {
                                            break;
                                        }
                                    }
                                }

                                Result::<_, esplora_client::Error>::Ok((index, related_txs))
                            }))
                        },
                    )
                    .collect::<Vec<_>>();

                let n_handles = handles.len();

                for handle in handles {
                    let (index, related_txs) = handle.join().unwrap()?; // TODO: don't unwrap
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

                if n_handles == 0 || empty_scripts >= stop_gap {
                    break;
                }
            }

            if let Some(last_active_index) = last_active_index {
                update.keychain.insert(keychain, last_active_index);
            }
        }

        for txid in txids.into_iter() {
            if update.graph.get_tx(txid).is_none() {
                match self.get_tx(&txid)? {
                    Some(tx) => {
                        let _ = update.graph.insert_tx(tx);
                    }
                    None => continue,
                }
            }
            match self.get_tx_status(&txid)? {
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
            ) = (self.get_tx(&op.txid)?, self.get_tx_status(&op.txid)?)
            {
                op_txs.push((tx, tx_status));
                if let Some(OutputStatus {
                    txid: Some(txid),
                    status: Some(spend_status),
                    ..
                }) = self.get_output_status(&op.txid, op.vout as _)?
                {
                    if let Some(spend_tx) = self.get_tx(&txid)? {
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
        while self.get_block_hash(update.tip.height())? != update.tip.hash() {
            let (new_tip, _) = construct_update_tip(self, Some(update.tip.clone()))?;
            make_anchor = crate::confirmation_time_anchor_maker(&new_tip);

            // Reconstruct graph with only transactions (no anchors).
            update.graph = TxGraph::new(update.graph.full_txs().map(|n| n.tx.clone()));
            update.tip = new_tip;

            // Re-fetch anchors.
            let anchors = update
                .graph
                .full_txs()
                .filter_map(|n| match self.get_tx_status(&n.txid) {
                    Err(err) => Some(Err(err)),
                    Ok(status) if status.confirmed => make_anchor(&status).map(|a| Ok((n.txid, a))),
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?;
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
fn construct_update_tip(
    client: &esplora_client::BlockingClient,
    prev_tip: Option<CheckPoint>,
) -> Result<(CheckPoint, Option<u32>), Error> {
    let new_tip_height = client.get_height()?;

    // If esplora returns a tip height that is lower than our previous tip, then checkpoints do not
    // need updating. We just return the previous tip and use that as the point of agreement.
    if let Some(prev_tip) = prev_tip.as_ref() {
        if new_tip_height < prev_tip.height() {
            return Ok((prev_tip.clone(), Some(prev_tip.height())));
        }
    }

    // Grab latest blocks from esplora atomically first. We assume that deeper blocks cannot be
    // reorged. This ensures that our checkpoint history is consistent.
    let mut new_blocks = {
        let heights = (0..new_tip_height).rev();
        let hashes = client
            .get_blocks(Some(new_tip_height))?
            .into_iter()
            .map(|b| b.id);
        heights.zip(hashes).collect::<BTreeMap<u32, BlockHash>>()
    };

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
                let hash = client.get_block_hash(cp_block.height)?;
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
