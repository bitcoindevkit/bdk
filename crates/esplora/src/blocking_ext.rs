use bdk_chain::bitcoin::{BlockHash, OutPoint, Script, Txid};
use bdk_chain::collections::BTreeMap;
use bdk_chain::local_chain::CheckPoint;
use bdk_chain::BlockId;
use bdk_chain::{keychain::LocalUpdate, ConfirmationTimeAnchor};
use esplora_client::{Error, OutputStatus, TxStatus};

use crate::map_confirmation_time_anchor;

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

        let (new_blocks, mut last_cp) = 'retry: loop {
            let new_tip = loop {
                let hash = self.get_tip_hash()?;
                let status = self.get_block_status(&hash)?;
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
                let hash = self.get_block_hash(cp_block.height)?;
                if hash == cp_block.hash {
                    agreement_cp = Some(cp);
                    break;
                }
                new_blocks.insert(cp_block.height, hash);
            }

            // check for tip changes
            // retry if there are changes to the tip
            let status = self.get_block_status(&new_tip.hash)?;
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
            last_cp = Some(
                CheckPoint::new_with_prev(BlockId { height, hash }, last_cp)
                    .expect("heights should not conflict"),
            );
        }

        let tip = last_cp.expect("must have atleast one checkpoint");
        let mut update = LocalUpdate::<K, ConfirmationTimeAnchor>::new(tip.clone());

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
                        let anchor = map_confirmation_time_anchor(&tx.status, &tip);

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
                tx_status @ TxStatus {
                    confirmed: true, ..
                } => {
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
                let anchor = map_confirmation_time_anchor(&status, &tip);

                let _ = update.graph.insert_tx(tx);
                if let Some(anchor) = anchor {
                    let _ = update.graph.insert_anchor(txid, anchor);
                }
            }
        }

        if tip.hash() != self.get_block_hash(tip.height())? {
            // A reorg occurred, so let's find out where all the txids we found are now in the chain
            let txids_found = update
                .graph
                .full_txs()
                .map(|tx_node| tx_node.txid)
                .collect::<Vec<_>>();
            let new_update = EsploraExt::scan_without_keychain(
                self,
                Some(tip),
                [],
                txids_found,
                [],
                parallel_requests,
            )?;
            update.tip = new_update.tip;
            update.graph = new_update.graph;
        }

        Ok(update)
    }
}
