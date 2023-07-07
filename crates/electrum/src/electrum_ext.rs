use bdk_chain::{
    bitcoin::{hashes::hex::FromHex, OutPoint, Script, Transaction, Txid},
    keychain::LocalUpdate,
    local_chain::CheckPoint,
    tx_graph::{self, TxGraph},
    Anchor, BlockId, ConfirmationHeightAnchor, ConfirmationTimeAnchor,
};
use electrum_client::{Client, ElectrumApi, Error, HeaderNotification};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Debug,
};

/// We assume that a block of this depth and deeper cannot be reorged.
const ASSUME_FINAL_DEPTH: u32 = 8;

/// Represents an update fetched from an Electrum server, but excludes full transactions.
///
/// To provide a complete update to [`TxGraph`], you'll need to call [`Self::missing_full_txs`] to
/// determine the full transactions missing from [`TxGraph`]. Then call [`Self::finalize`] to fetch
/// the full transactions from Electrum and finalize the update.
#[derive(Debug, Clone)]
pub struct ElectrumUpdate<K, A> {
    /// Map of [`Txid`]s to associated [`Anchor`]s.
    pub graph_update: HashMap<Txid, BTreeSet<A>>,
    /// The latest chain tip, as seen by the Electrum server.
    pub chain_update: CheckPoint,
    /// Last-used index update for [`KeychainTxOutIndex`](bdk_chain::keychain::KeychainTxOutIndex).
    pub keychain_update: BTreeMap<K, u32>,
}

impl<K, A: Anchor> ElectrumUpdate<K, A> {
    fn new(cp: CheckPoint) -> Self {
        Self {
            graph_update: HashMap::new(),
            chain_update: cp,
            keychain_update: BTreeMap::new(),
        }
    }

    /// Determine the full transactions that are missing from `graph`.
    ///
    /// Refer to [`ElectrumUpdate`].
    pub fn missing_full_txs<A2>(&self, graph: &TxGraph<A2>) -> Vec<Txid> {
        self.graph_update
            .keys()
            .filter(move |&&txid| graph.as_ref().get_tx(txid).is_none())
            .cloned()
            .collect()
    }

    /// Finalizes update with `missing` txids to fetch from `client`.
    ///
    /// Refer to [`ElectrumUpdate`].
    pub fn finalize(
        self,
        client: &Client,
        seen_at: Option<u64>,
        missing: Vec<Txid>,
    ) -> Result<LocalUpdate<K, A>, Error> {
        let new_txs = client.batch_transaction_get(&missing)?;
        let mut graph_update = TxGraph::<A>::new(new_txs);
        for (txid, anchors) in self.graph_update {
            if let Some(seen_at) = seen_at {
                let _ = graph_update.insert_seen_at(txid, seen_at);
            }
            for anchor in anchors {
                let _ = graph_update.insert_anchor(txid, anchor);
            }
        }
        Ok(LocalUpdate {
            keychain: self.keychain_update,
            graph: graph_update,
            tip: self.chain_update,
        })
    }
}

impl<K> ElectrumUpdate<K, ConfirmationHeightAnchor> {
    /// Finalizes the [`ElectrumUpdate`] with `new_txs` and anchors of type
    /// [`ConfirmationTimeAnchor`].
    ///
    /// **Note:** The confirmation time might not be precisely correct if there has been a reorg.
    /// Electrum's API intends that we use the merkle proof API, we should change `bdk_electrum` to
    /// use it.
    pub fn finalize_as_confirmation_time(
        self,
        client: &Client,
        seen_at: Option<u64>,
        missing: Vec<Txid>,
    ) -> Result<LocalUpdate<K, ConfirmationTimeAnchor>, Error> {
        let update = self.finalize(client, seen_at, missing)?;
        // client.batch_transaction_get(txid)

        let relevant_heights = {
            let mut visited_heights = HashSet::new();
            update
                .graph
                .all_anchors()
                .iter()
                .map(|(a, _)| a.confirmation_height_upper_bound())
                .filter(move |&h| visited_heights.insert(h))
                .collect::<Vec<_>>()
        };

        let height_to_time = relevant_heights
            .clone()
            .into_iter()
            .zip(
                client
                    .batch_block_header(relevant_heights)?
                    .into_iter()
                    .map(|bh| bh.time as u64),
            )
            .collect::<HashMap<u32, u64>>();

        let graph_additions = {
            let old_additions = TxGraph::default().determine_additions(&update.graph);
            tx_graph::Additions {
                txs: old_additions.txs,
                txouts: old_additions.txouts,
                last_seen: old_additions.last_seen,
                anchors: old_additions
                    .anchors
                    .into_iter()
                    .map(|(height_anchor, txid)| {
                        let confirmation_height = height_anchor.confirmation_height;
                        let confirmation_time = height_to_time[&confirmation_height];
                        let time_anchor = ConfirmationTimeAnchor {
                            anchor_block: height_anchor.anchor_block,
                            confirmation_height,
                            confirmation_time,
                        };
                        (time_anchor, txid)
                    })
                    .collect(),
            }
        };

        Ok(LocalUpdate {
            keychain: update.keychain,
            graph: {
                let mut graph = TxGraph::default();
                graph.apply_additions(graph_additions);
                graph
            },
            tip: update.tip,
        })
    }
}

/// Trait to extend [`Client`] functionality.
pub trait ElectrumExt<A> {
    /// Scan the blockchain (via electrum) for the data specified and returns a [`ElectrumUpdate`].
    ///
    /// - `prev_tip`: the most recent blockchain tip present locally
    /// - `keychain_spks`: keychains that we want to scan transactions for
    /// - `txids`: transactions for which we want updated [`Anchor`]s
    /// - `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to included in the update
    ///
    /// The scan for each keychain stops after a gap of `stop_gap` script pubkeys with no associated
    /// transactions. `batch_size` specifies the max number of script pubkeys to request for in a
    /// single batch request.
    fn scan<K: Ord + Clone>(
        &self,
        prev_tip: Option<CheckPoint>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<ElectrumUpdate<K, A>, Error>;

    /// Convenience method to call [`scan`] without requiring a keychain.
    ///
    /// [`scan`]: ElectrumExt::scan
    fn scan_without_keychain(
        &self,
        prev_tip: Option<CheckPoint>,
        misc_spks: impl IntoIterator<Item = Script>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        batch_size: usize,
    ) -> Result<ElectrumUpdate<(), A>, Error> {
        let spk_iter = misc_spks
            .into_iter()
            .enumerate()
            .map(|(i, spk)| (i as u32, spk));

        self.scan(
            prev_tip,
            [((), spk_iter)].into(),
            txids,
            outpoints,
            usize::MAX,
            batch_size,
        )
    }
}

impl ElectrumExt<ConfirmationHeightAnchor> for Client {
    fn scan<K: Ord + Clone>(
        &self,
        prev_tip: Option<CheckPoint>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<ElectrumUpdate<K, ConfirmationHeightAnchor>, Error> {
        let mut request_spks = keychain_spks
            .into_iter()
            .map(|(k, s)| (k, s.into_iter()))
            .collect::<BTreeMap<K, _>>();
        let mut scanned_spks = BTreeMap::<(K, u32), (Script, bool)>::new();

        let txids = txids.into_iter().collect::<Vec<_>>();
        let outpoints = outpoints.into_iter().collect::<Vec<_>>();

        let update = loop {
            let (tip, _) = construct_update_tip(self, prev_tip.clone())?;
            let mut update = ElectrumUpdate::<K, ConfirmationHeightAnchor>::new(tip.clone());
            let cps = update
                .chain_update
                .iter()
                .take(10)
                .map(|cp| (cp.height(), cp))
                .collect::<BTreeMap<u32, CheckPoint>>();

            if !request_spks.is_empty() {
                if !scanned_spks.is_empty() {
                    scanned_spks.append(&mut populate_with_spks(
                        self,
                        &cps,
                        &mut update,
                        &mut scanned_spks
                            .iter()
                            .map(|(i, (spk, _))| (i.clone(), spk.clone())),
                        stop_gap,
                        batch_size,
                    )?);
                }
                for (keychain, keychain_spks) in &mut request_spks {
                    scanned_spks.extend(
                        populate_with_spks(
                            self,
                            &cps,
                            &mut update,
                            keychain_spks,
                            stop_gap,
                            batch_size,
                        )?
                        .into_iter()
                        .map(|(spk_i, spk)| ((keychain.clone(), spk_i), spk)),
                    );
                }
            }

            populate_with_txids(self, &cps, &mut update, &mut txids.iter().cloned())?;

            let _txs =
                populate_with_outpoints(self, &cps, &mut update, &mut outpoints.iter().cloned())?;

            // check for reorgs during scan process
            let server_blockhash = self.block_header(tip.height() as usize)?.block_hash();
            if tip.hash() != server_blockhash {
                continue; // reorg
            }

            update.keychain_update = request_spks
                .into_keys()
                .filter_map(|k| {
                    scanned_spks
                        .range((k.clone(), u32::MIN)..=(k.clone(), u32::MAX))
                        .rev()
                        .find(|(_, (_, active))| *active)
                        .map(|((_, i), _)| (k, *i))
                })
                .collect::<BTreeMap<_, _>>();
            break update;
        };

        Ok(update)
    }
}

/// Return a [`CheckPoint`] of the latest tip, that connects with `prev_tip`.
fn construct_update_tip(
    client: &Client,
    prev_tip: Option<CheckPoint>,
) -> Result<(CheckPoint, Option<u32>), Error> {
    let HeaderNotification { height, .. } = client.block_headers_subscribe()?;
    let new_tip_height = height as u32;

    // If electrum returns a tip height that is lower than our previous tip, then checkpoints do
    // not need updating. We just return the previous tip and use that as the point of agreement.
    if let Some(prev_tip) = prev_tip.as_ref() {
        if new_tip_height < prev_tip.height() {
            return Ok((prev_tip.clone(), Some(prev_tip.height())));
        }
    }

    // Atomically fetch the latest `ASSUME_FINAL_DEPTH` count of blocks from Electrum. We use this
    // to construct our checkpoint update.
    let mut new_blocks = {
        let start_height = new_tip_height.saturating_sub(ASSUME_FINAL_DEPTH);
        let hashes = client
            .block_headers(start_height as _, ASSUME_FINAL_DEPTH as _)?
            .headers
            .into_iter()
            .map(|h| h.block_hash());
        (start_height..).zip(hashes).collect::<BTreeMap<u32, _>>()
    };

    // Find the "point of agreement" (if any).
    let agreement_cp = {
        let mut agreement_cp = Option::<CheckPoint>::None;
        for cp in prev_tip.iter().flat_map(CheckPoint::iter) {
            let cp_block = cp.block_id();
            let hash = match new_blocks.get(&cp_block.height) {
                Some(&hash) => hash,
                None => {
                    assert!(
                        new_tip_height >= cp_block.height,
                        "already checked that electrum's tip cannot be smaller"
                    );
                    let hash = client.block_header(cp_block.height as _)?.block_hash();
                    new_blocks.insert(cp_block.height, hash);
                    hash
                }
            };
            if hash == cp_block.hash {
                agreement_cp = Some(cp);
                break;
            }
        }
        agreement_cp
    };

    let agreement_height = agreement_cp.as_ref().map(CheckPoint::height);

    let new_tip = new_blocks
        .into_iter()
        // Prune `new_blocks` to only include blocks that are actually new.
        .filter(|(height, _)| Some(*height) > agreement_height)
        .map(|(height, hash)| BlockId { height, hash })
        .fold(agreement_cp, |prev_cp, block| {
            Some(match prev_cp {
                Some(cp) => cp.push(block).expect("must extend checkpoint"),
                None => CheckPoint::new(block),
            })
        })
        .expect("must have at least one checkpoint");

    Ok((new_tip, agreement_height))
}

/// A [tx status] comprises of a concatenation of `tx_hash:height:`s. We transform a single one of
/// these concatenations into a [`ConfirmationHeightAnchor`] if possible.
///
/// We use the lowest possible checkpoint as the anchor block (from `cps`). If an anchor block
/// cannot be found, or the transaction is unconfirmed, [`None`] is returned.
///
/// [tx status](https://electrumx-spesmilo.readthedocs.io/en/latest/protocol-basics.html#status)
fn determine_tx_anchor(
    cps: &BTreeMap<u32, CheckPoint>,
    raw_height: i32,
    txid: Txid,
) -> Option<ConfirmationHeightAnchor> {
    // The electrum API has a weird quirk where an unconfirmed transaction is presented with a
    // height of 0. To avoid invalid representation in our data structures, we manually set
    // transactions residing in the genesis block to have height 0, then interpret a height of 0 as
    // unconfirmed for all other transactions.
    if txid
        == Txid::from_hex("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
            .expect("must deserialize genesis coinbase txid")
    {
        let anchor_block = cps.values().next()?.block_id();
        return Some(ConfirmationHeightAnchor {
            anchor_block,
            confirmation_height: 0,
        });
    }
    match raw_height {
        h if h <= 0 => {
            debug_assert!(h == 0 || h == -1, "unexpected height ({}) from electrum", h);
            None
        }
        h => {
            let h = h as u32;
            let anchor_block = cps.range(h..).next().map(|(_, cp)| cp.block_id())?;
            if h > anchor_block.height {
                None
            } else {
                Some(ConfirmationHeightAnchor {
                    anchor_block,
                    confirmation_height: h,
                })
            }
        }
    }
}

fn populate_with_outpoints<K>(
    client: &Client,
    cps: &BTreeMap<u32, CheckPoint>,
    update: &mut ElectrumUpdate<K, ConfirmationHeightAnchor>,
    outpoints: &mut impl Iterator<Item = OutPoint>,
) -> Result<HashMap<Txid, Transaction>, Error> {
    let mut full_txs = HashMap::new();
    for outpoint in outpoints {
        let txid = outpoint.txid;
        let tx = client.transaction_get(&txid)?;
        debug_assert_eq!(tx.txid(), txid);
        let txout = match tx.output.get(outpoint.vout as usize) {
            Some(txout) => txout,
            None => continue,
        };
        // attempt to find the following transactions (alongside their chain positions), and
        // add to our sparsechain `update`:
        let mut has_residing = false; // tx in which the outpoint resides
        let mut has_spending = false; // tx that spends the outpoint
        for res in client.script_get_history(&txout.script_pubkey)? {
            if has_residing && has_spending {
                break;
            }

            if res.tx_hash == txid {
                if has_residing {
                    continue;
                }
                has_residing = true;
                full_txs.insert(res.tx_hash, tx.clone());
            } else {
                if has_spending {
                    continue;
                }
                let res_tx = match full_txs.get(&res.tx_hash) {
                    Some(tx) => tx,
                    None => {
                        let res_tx = client.transaction_get(&res.tx_hash)?;
                        full_txs.insert(res.tx_hash, res_tx);
                        full_txs.get(&res.tx_hash).expect("just inserted")
                    }
                };
                has_spending = res_tx
                    .input
                    .iter()
                    .any(|txin| txin.previous_output == outpoint);
                if !has_spending {
                    continue;
                }
            };

            let anchor = determine_tx_anchor(cps, res.height, res.tx_hash);

            let tx_entry = update.graph_update.entry(res.tx_hash).or_default();
            if let Some(anchor) = anchor {
                tx_entry.insert(anchor);
            }
        }
    }
    Ok(full_txs)
}

fn populate_with_txids<K>(
    client: &Client,
    cps: &BTreeMap<u32, CheckPoint>,
    update: &mut ElectrumUpdate<K, ConfirmationHeightAnchor>,
    txids: &mut impl Iterator<Item = Txid>,
) -> Result<(), Error> {
    for txid in txids {
        let tx = match client.transaction_get(&txid) {
            Ok(tx) => tx,
            Err(electrum_client::Error::Protocol(_)) => continue,
            Err(other_err) => return Err(other_err),
        };

        let spk = tx
            .output
            .get(0)
            .map(|txo| &txo.script_pubkey)
            .expect("tx must have an output");

        let anchor = match client
            .script_get_history(spk)?
            .into_iter()
            .find(|r| r.tx_hash == txid)
        {
            Some(r) => determine_tx_anchor(cps, r.height, txid),
            None => continue,
        };

        let tx_entry = update.graph_update.entry(txid).or_default();
        if let Some(anchor) = anchor {
            tx_entry.insert(anchor);
        }
    }
    Ok(())
}

fn populate_with_spks<K, I: Ord + Clone>(
    client: &Client,
    cps: &BTreeMap<u32, CheckPoint>,
    update: &mut ElectrumUpdate<K, ConfirmationHeightAnchor>,
    spks: &mut impl Iterator<Item = (I, Script)>,
    stop_gap: usize,
    batch_size: usize,
) -> Result<BTreeMap<I, (Script, bool)>, Error> {
    let mut unused_spk_count = 0_usize;
    let mut scanned_spks = BTreeMap::new();

    loop {
        let spks = (0..batch_size)
            .map_while(|_| spks.next())
            .collect::<Vec<_>>();
        if spks.is_empty() {
            return Ok(scanned_spks);
        }

        let spk_histories = client.batch_script_get_history(spks.iter().map(|(_, s)| s))?;

        for ((spk_index, spk), spk_history) in spks.into_iter().zip(spk_histories) {
            if spk_history.is_empty() {
                scanned_spks.insert(spk_index, (spk, false));
                unused_spk_count += 1;
                if unused_spk_count > stop_gap {
                    return Ok(scanned_spks);
                }
                continue;
            } else {
                scanned_spks.insert(spk_index, (spk, true));
                unused_spk_count = 0;
            }

            for tx in spk_history {
                let tx_entry = update.graph_update.entry(tx.tx_hash).or_default();
                if let Some(anchor) = determine_tx_anchor(cps, tx.height, tx.tx_hash) {
                    tx_entry.insert(anchor);
                }
            }
        }
    }
}
