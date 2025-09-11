//! Canonical view.

use crate::collections::HashMap;
use alloc::sync::Arc;
use core::{fmt, ops::RangeBounds};

use alloc::vec::Vec;

use bdk_core::BlockId;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, Txid};

use crate::{
    spk_txout::SpkTxOutIndex, tx_graph::TxNode, Anchor, Balance, CanonicalIter, CanonicalReason,
    CanonicalizationParams, ChainOracle, ChainPosition, FullTxOut, ObservedIn, TxGraph,
};

/// A single canonical transaction.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalViewTx<A> {
    /// Chain position.
    pub pos: ChainPosition<A>,
    /// Transaction ID.
    pub txid: Txid,
    /// The actual transaction.
    pub tx: Arc<Transaction>,
}

/// A view of canonical transactions.
#[derive(Debug)]
pub struct CanonicalView<A> {
    order: Vec<Txid>,
    txs: HashMap<Txid, (Arc<Transaction>, ChainPosition<A>)>,
    spends: HashMap<OutPoint, Txid>,
    tip: BlockId,
}

impl<A: Anchor> CanonicalView<A> {
    /// Create a canonical view.
    pub fn new<'g, C>(
        tx_graph: &'g TxGraph<A>,
        chain: &'g C,
        chain_tip: BlockId,
        params: CanonicalizationParams,
    ) -> Result<Self, C::Error>
    where
        C: ChainOracle,
    {
        fn find_direct_anchor<'g, A: Anchor, C: ChainOracle>(
            tx_node: &TxNode<'g, Arc<Transaction>, A>,
            chain: &C,
            chain_tip: BlockId,
        ) -> Result<Option<A>, C::Error> {
            tx_node
                .anchors
                .iter()
                .find_map(|a| -> Option<Result<A, C::Error>> {
                    match chain.is_block_in_chain(a.anchor_block(), chain_tip) {
                        Ok(Some(true)) => Some(Ok(a.clone())),
                        Ok(Some(false)) | Ok(None) => None,
                        Err(err) => Some(Err(err)),
                    }
                })
                .transpose()
        }

        let mut view = Self {
            tip: chain_tip,
            order: vec![],
            txs: HashMap::new(),
            spends: HashMap::new(),
        };

        for r in CanonicalIter::new(tx_graph, chain, chain_tip, params) {
            let (txid, tx, why) = r?;

            let tx_node = match tx_graph.get_tx_node(txid) {
                Some(tx_node) => tx_node,
                None => {
                    // TODO: Have the `CanonicalIter` return `TxNode`s.
                    debug_assert!(false, "tx node must exist!");
                    continue;
                }
            };

            view.order.push(txid);

            if !tx.is_coinbase() {
                view.spends
                    .extend(tx.input.iter().map(|txin| (txin.previous_output, txid)));
            }

            let pos = match why {
                CanonicalReason::Assumed { descendant } => match descendant {
                    Some(_) => match find_direct_anchor(&tx_node, chain, chain_tip)? {
                        Some(anchor) => ChainPosition::Confirmed {
                            anchor,
                            transitively: None,
                        },
                        None => ChainPosition::Unconfirmed {
                            first_seen: tx_node.first_seen,
                            last_seen: tx_node.last_seen,
                        },
                    },
                    None => ChainPosition::Unconfirmed {
                        first_seen: tx_node.first_seen,
                        last_seen: tx_node.last_seen,
                    },
                },
                CanonicalReason::Anchor { anchor, descendant } => match descendant {
                    Some(_) => match find_direct_anchor(&tx_node, chain, chain_tip)? {
                        Some(anchor) => ChainPosition::Confirmed {
                            anchor,
                            transitively: None,
                        },
                        None => ChainPosition::Confirmed {
                            anchor,
                            transitively: descendant,
                        },
                    },
                    None => ChainPosition::Confirmed {
                        anchor,
                        transitively: None,
                    },
                },
                CanonicalReason::ObservedIn { observed_in, .. } => match observed_in {
                    ObservedIn::Mempool(last_seen) => ChainPosition::Unconfirmed {
                        first_seen: tx_node.first_seen,
                        last_seen: Some(last_seen),
                    },
                    ObservedIn::Block(_) => ChainPosition::Unconfirmed {
                        first_seen: tx_node.first_seen,
                        last_seen: None,
                    },
                },
            };
            view.txs.insert(txid, (tx_node.tx, pos));
        }

        Ok(view)
    }

    /// Get a single canonical transaction.
    pub fn tx(&self, txid: Txid) -> Option<CanonicalViewTx<A>> {
        self.txs
            .get(&txid)
            .cloned()
            .map(|(tx, pos)| CanonicalViewTx { pos, txid, tx })
    }

    /// Get a single canonical txout.
    pub fn txout(&self, op: OutPoint) -> Option<FullTxOut<A>> {
        let (tx, pos) = self.txs.get(&op.txid)?;
        let vout: usize = op.vout.try_into().ok()?;
        let txout = tx.output.get(vout)?;
        let spent_by = self.spends.get(&op).map(|spent_by_txid| {
            let (_, spent_by_pos) = &self.txs[spent_by_txid];
            (spent_by_pos.clone(), *spent_by_txid)
        });
        Some(FullTxOut {
            chain_position: pos.clone(),
            outpoint: op,
            txout: txout.clone(),
            spent_by,
            is_on_coinbase: tx.is_coinbase(),
        })
    }

    /// Ordered transactions.
    pub fn txs(
        &self,
    ) -> impl ExactSizeIterator<Item = CanonicalViewTx<A>> + DoubleEndedIterator + '_ {
        self.order.iter().map(|&txid| {
            let (tx, pos) = self.txs[&txid].clone();
            CanonicalViewTx { pos, txid, tx }
        })
    }

    /// Get a filtered list of outputs from the given `outpoints`.
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`O`) for convenience. If `O` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    pub fn filter_outpoints<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
    ) -> impl Iterator<Item = (O, FullTxOut<A>)> + 'v {
        outpoints
            .into_iter()
            .filter_map(|(op_i, op)| Some((op_i, self.txout(op)?)))
    }

    /// Get a filtered list of unspent outputs (UTXOs) from the given `outpoints`
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`O`) for convenience. If `O` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    pub fn filter_unspent_outpoints<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
    ) -> impl Iterator<Item = (O, FullTxOut<A>)> + 'v {
        self.filter_outpoints(outpoints)
            .filter(|(_, txo)| txo.spent_by.is_none())
    }

    /// Get the total balance of `outpoints`.
    ///
    /// The output of `trust_predicate` should return `true` for scripts that we trust.
    ///
    /// `outpoints` is a list of outpoints we are interested in, coupled with an outpoint identifier
    /// (`O`) for convenience. If `O` is not necessary, the caller can use `()`, or
    /// [`Iterator::enumerate`] over a list of [`OutPoint`]s.
    pub fn balance<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
        mut trust_predicate: impl FnMut(&O, ScriptBuf) -> bool,
    ) -> Balance {
        let mut immature = Amount::ZERO;
        let mut trusted_pending = Amount::ZERO;
        let mut untrusted_pending = Amount::ZERO;
        let mut confirmed = Amount::ZERO;

        for (spk_i, txout) in self.filter_unspent_outpoints(outpoints) {
            match &txout.chain_position {
                ChainPosition::Confirmed { .. } => {
                    if txout.is_confirmed_and_spendable(self.tip.height) {
                        confirmed += txout.txout.value;
                    } else if !txout.is_mature(self.tip.height) {
                        immature += txout.txout.value;
                    }
                }
                ChainPosition::Unconfirmed { .. } => {
                    if trust_predicate(&spk_i, txout.txout.script_pubkey) {
                        trusted_pending += txout.txout.value;
                    } else {
                        untrusted_pending += txout.txout.value;
                    }
                }
            }
        }

        Balance {
            immature,
            trusted_pending,
            untrusted_pending,
            confirmed,
        }
    }

    /// List txids that are expected to exist under the given spks.
    ///
    /// This is used to fill
    /// [`SyncRequestBuilder::expected_spk_txids`](bdk_core::spk_client::SyncRequestBuilder::expected_spk_txids).
    ///
    ///
    /// The spk index range can be constrained with `range`.
    pub fn list_expected_spk_txids<'v, I>(
        &'v self,
        indexer: &'v impl AsRef<SpkTxOutIndex<I>>,
        spk_index_range: impl RangeBounds<I> + 'v,
    ) -> impl Iterator<Item = (ScriptBuf, Txid)> + 'v
    where
        I: fmt::Debug + Clone + Ord + 'v,
    {
        let indexer = indexer.as_ref();
        self.txs().flat_map(move |c_tx| -> Vec<_> {
            let range = &spk_index_range;
            let relevant_spks = indexer.relevant_spks_of_tx(&c_tx.tx);
            relevant_spks
                .into_iter()
                .filter(|(i, _)| range.contains(i))
                .map(|(_, spk)| (spk, c_tx.txid))
                .collect()
        })
    }
}
