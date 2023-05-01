use bdk_chain::{
    bitcoin::{hashes::hex::FromHex, BlockHash, OutPoint, Script, Transaction, Txid},
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{DerivationAdditions, KeychainTxOutIndex},
    local_chain::{self, LocalChain, UpdateNotConnectedError},
    tx_graph::{self, TxGraph},
    Anchor, Append, BlockId, ConfirmationHeightAnchor, ConfirmationTimeAnchor,
};
use electrum_client::{Client, ElectrumApi, Error};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Debug,
};

use crate::InternalError;

#[derive(Debug, Clone)]
pub struct ElectrumUpdate<G, K> {
    pub graph_update: G,
    pub chain_update: LocalChain,
    pub keychain_update: BTreeMap<K, u32>,
}

impl<G: Default, K> Default for ElectrumUpdate<G, K> {
    fn default() -> Self {
        Self {
            graph_update: Default::default(),
            chain_update: Default::default(),
            keychain_update: Default::default(),
        }
    }
}

pub type IntermediaryElectrumUpdate<A, K> = ElectrumUpdate<HashMap<Txid, BTreeSet<A>>, K>;

impl<'a, A: Anchor, K> IntermediaryElectrumUpdate<A, K> {
    pub fn missing_full_txs<A2>(
        &'a self,
        graph: &'a TxGraph<A2>,
    ) -> impl Iterator<Item = &'a Txid> + 'a {
        self.graph_update
            .keys()
            .filter(move |&&txid| graph.as_ref().get_tx(txid).is_none())
    }

    pub fn finalize<T>(self, seen_at: Option<u64>, new_txs: T) -> FinalElectrumUpdate<A, K>
    where
        T: IntoIterator<Item = Transaction>,
    {
        let mut graph_update = TxGraph::<A>::new(new_txs);
        for (txid, anchors) in self.graph_update {
            if let Some(seen_at) = seen_at {
                let _ = graph_update.insert_seen_at(txid, seen_at);
            }
            for anchor in anchors {
                let _ = graph_update.insert_anchor(txid, anchor);
            }
        }
        dbg!(graph_update.full_txs().count());
        FinalElectrumUpdate {
            graph_update,
            chain_update: self.chain_update,
            keychain_update: self.keychain_update,
        }
    }
}

pub type FinalElectrumUpdate<A, K> = ElectrumUpdate<TxGraph<A>, K>;

impl<K: Ord + Clone + Debug> FinalElectrumUpdate<ConfirmationHeightAnchor, K> {
    pub fn into_confirmation_time_update(
        self,
        client: &Client,
    ) -> Result<FinalElectrumUpdate<ConfirmationTimeAnchor, K>, Error> {
        let relevant_heights = {
            let mut visited_heights = HashSet::new();
            self.graph_update
                .all_anchors()
                .map(|(a, _)| a.confirmation_height_upper_bound())
                .filter(move |&h| visited_heights.insert(h))
                .collect::<Vec<_>>()
        };

        // [TODO] We need to check whether block hashes is "anchored" to our anchor.
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
            let old_additions = TxGraph::default().determine_additions(&self.graph_update);
            tx_graph::Additions {
                tx: old_additions.tx,
                txout: old_additions.txout,
                last_seen: old_additions.last_seen,
                anchors: old_additions
                    .anchors
                    .into_iter()
                    .map(|(height_anchor, txid)| {
                        let confirmation_height = dbg!(height_anchor
                            .confirmation_height
                            .expect("must have confirmation height"));
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

        Ok(FinalElectrumUpdate {
            graph_update: {
                let mut graph = TxGraph::default();
                graph.apply_additions(graph_additions);
                graph
            },
            chain_update: self.chain_update,
            keychain_update: self.keychain_update,
        })
    }
}

impl<A: Anchor, K: Ord + Clone + Debug> FinalElectrumUpdate<A, K> {
    pub fn apply(
        self,
        indexed_graph: &mut IndexedTxGraph<A, KeychainTxOutIndex<K>>,
        chain: &mut LocalChain,
    ) -> Result<
        (
            IndexedAdditions<A, DerivationAdditions<K>>,
            local_chain::ChangeSet,
        ),
        UpdateNotConnectedError,
    > {
        let (_, derivation_additions) = indexed_graph
            .index
            .reveal_to_target_multi(&self.keychain_update);

        let additions = {
            let mut additions = indexed_graph.apply_update(self.graph_update);
            additions.index_additions.append(derivation_additions);
            additions
        };

        let changeset = chain.apply_update(self.chain_update)?;

        Ok((additions, changeset))
    }
}

#[cfg(feature = "wallet")]
impl FinalElectrumUpdate<ConfirmationTimeAnchor, bdk::KeychainKind> {
    pub fn apply_to_tracker(
        self,
        tracker: &mut bdk::wallet::Tracker,
    ) -> Result<bdk::wallet::ChangeSet, UpdateNotConnectedError> {
        Ok(bdk::wallet::ChangeSet {
            indexed_additions: {
                let mut additions = tracker.indexed_graph.apply_update(self.graph_update);
                let (_, derivation_additions) = tracker
                    .indexed_graph
                    .index
                    .reveal_to_target_multi(&self.keychain_update);
                additions.index_additions.append(derivation_additions);
                additions
            },
            chain_changeset: tracker.chain.apply_update(self.chain_update)?,
        })
    }
}

pub trait ElectrumExt<A> {
    fn get_tip(&self) -> Result<(u32, BlockHash), Error>;

    fn scan<K: Ord + Clone>(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<IntermediaryElectrumUpdate<A, K>, Error>;

    fn scan_without_keychain(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        misc_spks: impl IntoIterator<Item = Script>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        batch_size: usize,
    ) -> Result<IntermediaryElectrumUpdate<A, ()>, Error> {
        let spk_iter = misc_spks
            .into_iter()
            .enumerate()
            .map(|(i, spk)| (i as u32, spk));

        self.scan(
            local_chain,
            [((), spk_iter)].into(),
            txids,
            outpoints,
            usize::MAX,
            batch_size,
        )
    }
}

impl ElectrumExt<ConfirmationHeightAnchor> for Client {
    fn get_tip(&self) -> Result<(u32, BlockHash), Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?
        self.block_headers_subscribe()
            .map(|data| (data.height as u32, data.header.block_hash()))
    }

    fn scan<K: Ord + Clone>(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        keychain_spks: BTreeMap<K, impl IntoIterator<Item = (u32, Script)>>,
        txids: impl IntoIterator<Item = Txid>,
        outpoints: impl IntoIterator<Item = OutPoint>,
        stop_gap: usize,
        batch_size: usize,
    ) -> Result<IntermediaryElectrumUpdate<ConfirmationHeightAnchor, K>, Error> {
        let mut request_spks = keychain_spks
            .into_iter()
            .map(|(k, s)| (k, s.into_iter()))
            .collect::<BTreeMap<K, _>>();
        let mut scanned_spks = BTreeMap::<(K, u32), (Script, bool)>::new();

        let txids = txids.into_iter().collect::<Vec<_>>();
        let outpoints = outpoints.into_iter().collect::<Vec<_>>();

        let update = loop {
            let mut update = IntermediaryElectrumUpdate::<ConfirmationHeightAnchor, K> {
                chain_update: prepare_chain_update(self, local_chain)?,
                ..Default::default()
            };
            let anchor_block = update
                .chain_update
                .tip()
                .expect("must have atleast one block");

            if !request_spks.is_empty() {
                if !scanned_spks.is_empty() {
                    let mut scanned_spk_iter = scanned_spks
                        .iter()
                        .map(|(i, (spk, _))| (i.clone(), spk.clone()));
                    match populate_with_spks(
                        self,
                        anchor_block,
                        &mut update,
                        &mut scanned_spk_iter,
                        stop_gap,
                        batch_size,
                    ) {
                        Err(InternalError::Reorg) => continue,
                        Err(InternalError::ElectrumError(e)) => return Err(e),
                        Ok(mut spks) => scanned_spks.append(&mut spks),
                    };
                }
                for (keychain, keychain_spks) in &mut request_spks {
                    match populate_with_spks(
                        self,
                        anchor_block,
                        &mut update,
                        keychain_spks,
                        stop_gap,
                        batch_size,
                    ) {
                        Err(InternalError::Reorg) => continue,
                        Err(InternalError::ElectrumError(e)) => return Err(e),
                        Ok(spks) => scanned_spks.extend(
                            spks.into_iter()
                                .map(|(spk_i, spk)| ((keychain.clone(), spk_i), spk)),
                        ),
                    };
                }
            }

            match populate_with_txids(self, anchor_block, &mut update, &mut txids.iter().cloned()) {
                Err(InternalError::Reorg) => continue,
                Err(InternalError::ElectrumError(e)) => return Err(e),
                Ok(_) => {}
            }

            match populate_with_outpoints(
                self,
                anchor_block,
                &mut update,
                &mut outpoints.iter().cloned(),
            ) {
                Err(InternalError::Reorg) => continue,
                Err(InternalError::ElectrumError(e)) => return Err(e),
                Ok(_txs) => { /* [TODO] cache full txs to reduce bandwidth */ }
            }

            // check for reorgs during scan process
            let server_blockhash = self
                .block_header(anchor_block.height as usize)?
                .block_hash();
            if anchor_block.hash != server_blockhash {
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

/// Prepare an update "template" based on the checkpoints of the `local_chain`.
fn prepare_chain_update(
    client: &Client,
    local_chain: &BTreeMap<u32, BlockHash>,
) -> Result<LocalChain, Error> {
    let mut update = LocalChain::default();

    // Find the local chain block that is still there so our update can connect to the local chain.
    for (&existing_height, &existing_hash) in local_chain.iter().rev() {
        // TODO: a batch request may be safer, as a reorg that happens when we are obtaining
        //       `block_header`s will result in inconsistencies
        let current_hash = client.block_header(existing_height as usize)?.block_hash();
        let _ = update
            .insert_block(BlockId {
                height: existing_height,
                hash: current_hash,
            })
            .expect("This never errors because we are working with a fresh chain");

        if current_hash == existing_hash {
            break;
        }
    }

    // Insert the new tip so new transactions will be accepted into the sparsechain.
    let tip = {
        let (height, hash) = crate::get_tip(client)?;
        BlockId { height, hash }
    };
    if update.insert_block(tip).is_err() {
        // There has been a re-org before we even begin scanning addresses.
        // Just recursively call (this should never happen).
        return prepare_chain_update(client, local_chain);
    }

    Ok(update)
}

fn determine_tx_anchor(
    anchor_block: BlockId,
    raw_height: i32,
    txid: Txid,
) -> Option<ConfirmationHeightAnchor> {
    if txid
        == Txid::from_hex("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
            .expect("must deserialize genesis coinbase txid")
    {
        return Some(ConfirmationHeightAnchor {
            anchor_block,
            confirmation_height: Some(0),
        });
    }
    match raw_height {
        h if h <= 0 => {
            debug_assert!(h == 0 || h == -1, "unexpected height ({}) from electrum", h);
            None
        }
        h => {
            let h = h as u32;
            if h > anchor_block.height {
                None
            } else {
                Some(ConfirmationHeightAnchor {
                    anchor_block,
                    confirmation_height: Some(h),
                })
            }
        }
    }
}

fn populate_with_outpoints<K>(
    client: &Client,
    anchor_block: BlockId,
    update: &mut IntermediaryElectrumUpdate<ConfirmationHeightAnchor, K>,
    outpoints: &mut impl Iterator<Item = OutPoint>,
) -> Result<HashMap<Txid, Transaction>, InternalError> {
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

            let anchor = determine_tx_anchor(anchor_block, res.height, res.tx_hash);

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
    anchor_block: BlockId,
    update: &mut IntermediaryElectrumUpdate<ConfirmationHeightAnchor, K>,
    txids: &mut impl Iterator<Item = Txid>,
) -> Result<(), InternalError> {
    for txid in txids {
        let tx = match client.transaction_get(&txid) {
            Ok(tx) => tx,
            Err(electrum_client::Error::Protocol(_)) => continue,
            Err(other_err) => return Err(other_err.into()),
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
            Some(r) => determine_tx_anchor(anchor_block, r.height, txid),
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
    anchor_block: BlockId,
    update: &mut IntermediaryElectrumUpdate<ConfirmationHeightAnchor, K>,
    spks: &mut impl Iterator<Item = (I, Script)>,
    stop_gap: usize,
    batch_size: usize,
) -> Result<BTreeMap<I, (Script, bool)>, InternalError> {
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
                if let Some(anchor) = determine_tx_anchor(anchor_block, tx.height, tx.tx_hash) {
                    tx_entry.insert(anchor);
                }
            }
        }
    }
}
