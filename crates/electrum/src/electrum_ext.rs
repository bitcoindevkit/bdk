use bdk_chain::{
    bitcoin::{OutPoint, ScriptBuf, Transaction, Txid},
    local_chain::{self, CheckPoint},
    tx_graph::{self, TxGraph},
    Anchor, BlockId, ConfirmationHeightAnchor, ConfirmationTimeHeightAnchor,
};
use electrum_client::{Client, ElectrumApi, Error, HeaderNotification};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Debug,
    str::FromStr,
};

/// We include a chain suffix of a certain length for the purpose of robustness.
const CHAIN_SUFFIX_LENGTH: u32 = 8;

/// Represents updates fetched from an Electrum server, but excludes full transactions.
///
/// To provide a complete update to [`TxGraph`], you'll need to call [`Self::missing_full_txs`] to
/// determine the full transactions missing from [`TxGraph`]. Then call [`Self::into_tx_graph`] to
/// fetch the full transactions from Electrum and finalize the update.
#[derive(Debug, Default, Clone)]
pub struct RelevantTxids(HashMap<Txid, BTreeSet<ConfirmationHeightAnchor>>);

impl RelevantTxids {
    /// Determine the full transactions that are missing from `graph`.
    ///
    /// Refer to [`RelevantTxids`] for more details.
    pub fn missing_full_txs<A: Anchor>(&self, graph: &TxGraph<A>) -> Vec<Txid> {
        self.0
            .keys()
            .filter(move |&&txid| graph.as_ref().get_tx(txid).is_none())
            .cloned()
            .collect()
    }

    /// Finalizes the [`TxGraph`] update by fetching `missing` txids from the `client`.
    ///
    /// Refer to [`RelevantTxids`] for more details.
    pub fn into_tx_graph(
        self,
        client: &Client,
        seen_at: Option<u64>,
        missing: Vec<Txid>,
    ) -> Result<TxGraph<ConfirmationHeightAnchor>, Error> {
        let new_txs = client.batch_transaction_get(&missing)?;
        let mut graph = TxGraph::<ConfirmationHeightAnchor>::new(new_txs);
        for (txid, anchors) in self.0 {
            if let Some(seen_at) = seen_at {
                let _ = graph.insert_seen_at(txid, seen_at);
            }
            for anchor in anchors {
                let _ = graph.insert_anchor(txid, anchor);
            }
        }
        Ok(graph)
    }

    /// Finalizes the update by fetching `missing` txids from the `client`, where the
    /// resulting [`TxGraph`] has anchors of type [`ConfirmationTimeHeightAnchor`].
    ///
    /// Refer to [`RelevantTxids`] for more details.
    ///
    /// **Note:** The confirmation time might not be precisely correct if there has been a reorg.
    // Electrum's API intends that we use the merkle proof API, we should change `bdk_electrum` to
    // use it.
    pub fn into_confirmation_time_tx_graph(
        self,
        client: &Client,
        seen_at: Option<u64>,
        missing: Vec<Txid>,
    ) -> Result<TxGraph<ConfirmationTimeHeightAnchor>, Error> {
        let graph = self.into_tx_graph(client, seen_at, missing)?;

        let relevant_heights = {
            let mut visited_heights = HashSet::new();
            graph
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

        let graph_changeset = {
            let old_changeset = TxGraph::default().apply_update(graph);
            tx_graph::ChangeSet {
                txs: old_changeset.txs,
                txouts: old_changeset.txouts,
                last_seen: old_changeset.last_seen,
                anchors: old_changeset
                    .anchors
                    .into_iter()
                    .map(|(height_anchor, txid)| {
                        let confirmation_height = height_anchor.confirmation_height;
                        let confirmation_time = height_to_time[&confirmation_height];
                        let time_anchor = ConfirmationTimeHeightAnchor {
                            anchor_block: height_anchor.anchor_block,
                            confirmation_height,
                            confirmation_time,
                        };
                        (time_anchor, txid)
                    })
                    .collect(),
            }
        };

        let mut new_graph = TxGraph::default();
        new_graph.apply_changeset(graph_changeset);
        Ok(new_graph)
    }
}

/// Combination of chain and transactions updates from electrum
///
/// We have to update the chain and the txids at the same time since we anchor the txids to
/// the same chain tip that we check before and after we gather the txids.
#[derive(Debug)]
pub struct ElectrumUpdate {
    /// Chain update
    pub chain_update: local_chain::Update,
    /// Transaction updates from electrum
    pub relevant_txids: RelevantTxids,
}

/// Trait to extend [`Client`] functionality.
pub trait ElectrumExt {
    /// Full scan the keychain scripts specified with the blockchain (via an Electrum client) and
    /// returns updates for [`bdk_chain`] data structures.
    ///
    /// - `prev_tip`: the most recent blockchain tip present locally
    /// - `keychain_spks`: keychains that we want to scan transactions for
    ///
    /// The full scan for each keychain stops after a gap of `stop_gap` script pubkeys with no associated
    /// transactions. `batch_size` specifies the max number of script pubkeys to request for in a
    /// single batch request.
    fn full_scan<K: Ord + Clone>(
        &self,
        prev_tip: CheckPoint,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, ScriptBuf)>>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<(ElectrumUpdate, BTreeMap<K, u32>), Error>;

    /// Sync a set of scripts with the blockchain (via an Electrum client) for the data specified
    /// and returns updates for [`bdk_chain`] data structures.
    ///
    /// - `prev_tip`: the most recent blockchain tip present locally
    /// - `misc_spks`: an iterator of scripts we want to sync transactions for
    /// - `txids`: transactions for which we want updated [`Anchor`]s
    /// - `outpoints`: transactions associated with these outpoints (residing, spending) that we
    ///     want to include in the update
    ///
    /// `batch_size` specifies the max number of script pubkeys to request for in a single batch
    /// request.
    ///
    /// If the scripts to sync are unknown, such as when restoring or importing a keychain that
    /// may include scripts that have been used, use [`full_scan`] with the keychain.
    ///
    /// [`full_scan`]: ElectrumExt::full_scan
    fn sync(
        &self,
        prev_tip: CheckPoint,
        misc_spks: impl IntoIterator<Item = ScriptBuf>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        batch_size: usize,
    ) -> Result<ElectrumUpdate, Error>;
}

impl<A: ElectrumApi> ElectrumExt for A {
    fn full_scan<K: Ord + Clone>(
        &self,
        prev_tip: CheckPoint,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, ScriptBuf)>>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<(ElectrumUpdate, BTreeMap<K, u32>), Error> {
        let mut request_spks = keychain_spks
            .into_iter()
            .map(|(k, s)| (k, s.into_iter()))
            .collect::<BTreeMap<K, _>>();
        let mut scanned_spks = BTreeMap::<(K, u32), (ScriptBuf, bool)>::new();

        let (electrum_update, keychain_update) = loop {
            let (tip, _) = construct_update_tip(self, prev_tip.clone())?;
            let mut relevant_txids = RelevantTxids::default();
            let cps = tip
                .iter()
                .take(10)
                .map(|cp| (cp.height(), cp))
                .collect::<BTreeMap<u32, CheckPoint>>();

            if !request_spks.is_empty() {
                if !scanned_spks.is_empty() {
                    scanned_spks.append(&mut populate_with_spks(
                        self,
                        &cps,
                        &mut relevant_txids,
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
                            &mut relevant_txids,
                            keychain_spks,
                            stop_gap,
                            batch_size,
                        )?
                        .into_iter()
                        .map(|(spk_i, spk)| ((keychain.clone(), spk_i), spk)),
                    );
                }
            }

            // check for reorgs during scan process
            let server_blockhash = self.block_header(tip.height() as usize)?.block_hash();
            if tip.hash() != server_blockhash {
                continue; // reorg
            }

            let chain_update = local_chain::Update {
                tip,
                introduce_older_blocks: true,
            };

            let keychain_update = request_spks
                .into_keys()
                .filter_map(|k| {
                    scanned_spks
                        .range((k.clone(), u32::MIN)..=(k.clone(), u32::MAX))
                        .rev()
                        .find(|(_, (_, active))| *active)
                        .map(|((_, i), _)| (k, *i))
                })
                .collect::<BTreeMap<_, _>>();

            break (
                ElectrumUpdate {
                    chain_update,
                    relevant_txids,
                },
                keychain_update,
            );
        };

        Ok((electrum_update, keychain_update))
    }

    fn sync(
        &self,
        prev_tip: CheckPoint,
        misc_spks: impl IntoIterator<Item = ScriptBuf>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        batch_size: usize,
    ) -> Result<ElectrumUpdate, Error> {
        let spk_iter = misc_spks
            .into_iter()
            .enumerate()
            .map(|(i, spk)| (i as u32, spk));

        let (mut electrum_update, _) = self.full_scan(
            prev_tip.clone(),
            [((), spk_iter)].into(),
            usize::MAX,
            batch_size,
        )?;

        let (tip, _) = construct_update_tip(self, prev_tip)?;
        let cps = tip
            .iter()
            .take(10)
            .map(|cp| (cp.height(), cp))
            .collect::<BTreeMap<u32, CheckPoint>>();

        populate_with_txids(self, &cps, &mut electrum_update.relevant_txids, txids)?;

        let _txs =
            populate_with_outpoints(self, &cps, &mut electrum_update.relevant_txids, outpoints)?;

        Ok(electrum_update)
    }
}

/// Return a [`CheckPoint`] of the latest tip, that connects with `prev_tip`.
fn construct_update_tip(
    client: &impl ElectrumApi,
    prev_tip: CheckPoint,
) -> Result<(CheckPoint, Option<u32>), Error> {
    let HeaderNotification { height, .. } = client.block_headers_subscribe()?;
    let new_tip_height = height as u32;

    // If electrum returns a tip height that is lower than our previous tip, then checkpoints do
    // not need updating. We just return the previous tip and use that as the point of agreement.
    if new_tip_height < prev_tip.height() {
        return Ok((prev_tip.clone(), Some(prev_tip.height())));
    }

    // Atomically fetch the latest `CHAIN_SUFFIX_LENGTH` count of blocks from Electrum. We use this
    // to construct our checkpoint update.
    let mut new_blocks = {
        let start_height = new_tip_height.saturating_sub(CHAIN_SUFFIX_LENGTH - 1);
        let hashes = client
            .block_headers(start_height as _, CHAIN_SUFFIX_LENGTH as _)?
            .headers
            .into_iter()
            .map(|h| h.block_hash());
        (start_height..).zip(hashes).collect::<BTreeMap<u32, _>>()
    };

    // Find the "point of agreement" (if any).
    let agreement_cp = {
        let mut agreement_cp = Option::<CheckPoint>::None;
        for cp in prev_tip.iter() {
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
        == Txid::from_str("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
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

fn populate_with_outpoints(
    client: &impl ElectrumApi,
    cps: &BTreeMap<u32, CheckPoint>,
    relevant_txids: &mut RelevantTxids,
    outpoints: impl IntoIterator<Item = OutPoint>,
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
            let tx_entry = relevant_txids.0.entry(res.tx_hash).or_default();
            if let Some(anchor) = anchor {
                tx_entry.insert(anchor);
            }
        }
    }
    Ok(full_txs)
}

fn populate_with_txids(
    client: &impl ElectrumApi,
    cps: &BTreeMap<u32, CheckPoint>,
    relevant_txids: &mut RelevantTxids,
    txids: impl IntoIterator<Item = Txid>,
) -> Result<(), Error> {
    for txid in txids {
        let tx = match client.transaction_get(&txid) {
            Ok(tx) => tx,
            Err(electrum_client::Error::Protocol(_)) => continue,
            Err(other_err) => return Err(other_err),
        };

        let spk = tx
            .output
            .first()
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

        let tx_entry = relevant_txids.0.entry(txid).or_default();
        if let Some(anchor) = anchor {
            tx_entry.insert(anchor);
        }
    }
    Ok(())
}

fn populate_with_spks<I: Ord + Clone>(
    client: &impl ElectrumApi,
    cps: &BTreeMap<u32, CheckPoint>,
    relevant_txids: &mut RelevantTxids,
    spks: &mut impl Iterator<Item = (I, ScriptBuf)>,
    stop_gap: usize,
    batch_size: usize,
) -> Result<BTreeMap<I, (ScriptBuf, bool)>, Error> {
    let mut unused_spk_count = 0_usize;
    let mut scanned_spks = BTreeMap::new();

    loop {
        let spks = (0..batch_size)
            .map_while(|_| spks.next())
            .collect::<Vec<_>>();
        if spks.is_empty() {
            return Ok(scanned_spks);
        }

        let spk_histories =
            client.batch_script_get_history(spks.iter().map(|(_, s)| s.as_script()))?;

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
                let tx_entry = relevant_txids.0.entry(tx.tx_hash).or_default();
                if let Some(anchor) = determine_tx_anchor(cps, tx.height, tx.tx_hash) {
                    tx_entry.insert(anchor);
                }
            }
        }
    }
}
