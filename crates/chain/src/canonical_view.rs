//! Canonical view of transactions and unspent outputs.
//!
//! This module provides [`CanonicalView`], a utility for obtaining a canonical (ordered and
//! conflict-resolved) view of transactions from a [`TxGraph`].
//!
//! ## Example
//!
//! ```
//! # use bdk_chain::{CanonicalView, TxGraph, CanonicalizationParams, local_chain::LocalChain};
//! # use bdk_core::BlockId;
//! # use bitcoin::hashes::Hash;
//! # let tx_graph = TxGraph::<BlockId>::default();
//! # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
//! # let chain_tip = chain.tip().block_id();
//! let params = CanonicalizationParams::default();
//! let view = CanonicalView::new(&tx_graph, &chain, chain_tip, params).unwrap();
//!
//! // Iterate over canonical transactions
//! for tx in view.txs() {
//!     println!("Transaction {}: {:?}", tx.txid, tx.pos);
//! }
//! ```

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

/// A single canonical transaction with its chain position.
///
/// This struct represents a transaction that has been determined to be canonical (not
/// conflicted). It includes the transaction itself along with its position in the chain (confirmed
/// or unconfirmed).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalTx<A> {
    /// The position of this transaction in the chain.
    ///
    /// This indicates whether the transaction is confirmed (and at what height) or
    /// unconfirmed (most likely pending in the mempool).
    pub pos: ChainPosition<A>,
    /// The transaction ID (hash) of this transaction.
    pub txid: Txid,
    /// The full transaction.
    pub tx: Arc<Transaction>,
}

/// A view of canonical transactions from a [`TxGraph`].
///
/// `CanonicalView` provides an ordered, conflict-resolved view of transactions. It determines
/// which transactions are canonical (non-conflicted) based on the current chain state and
/// provides methods to query transaction data, unspent outputs, and balances.
///
/// The view maintains:
/// - An ordered list of canonical transactions (WIP)
/// - A mapping of outpoints to the transactions that spend them
/// - The chain tip used for canonicalization
#[derive(Debug)]
pub struct CanonicalView<A> {
    /// Ordered list of transaction IDs in canonical order.
    order: Vec<Txid>,
    /// Map of transaction IDs to their transaction data and chain position.
    txs: HashMap<Txid, (Arc<Transaction>, ChainPosition<A>)>,
    /// Map of outpoints to the transaction ID that spends them.
    spends: HashMap<OutPoint, Txid>,
    /// The chain tip at the time this view was created.
    tip: BlockId,
}

impl<A: Anchor> CanonicalView<A> {
    /// Create a new canonical view from a transaction graph.
    ///
    /// This constructor analyzes the given [`TxGraph`] and creates a canonical view of all
    /// transactions, resolving conflicts and ordering them according to their chain position.
    ///
    /// # Arguments
    ///
    /// * `tx_graph` - The transaction graph containing all known transactions
    /// * `chain` - A chain oracle for determining block inclusion
    /// * `chain_tip` - The current chain tip to use for canonicalization
    /// * `params` - Parameters controlling the canonicalization process
    ///
    /// # Returns
    ///
    /// Returns `Ok(CanonicalView)` on success, or an error if the chain oracle fails.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, CanonicalizationParams, local_chain::LocalChain};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// let chain_tip = chain.tip().block_id();
    /// let params = CanonicalizationParams::default();
    ///
    /// let view = CanonicalView::new(&tx_graph, &chain, chain_tip, params)?;
    /// # Ok::<_, Box<dyn std::error::Error>>(())
    /// ```
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

    /// Get a single canonical transaction by its transaction ID.
    ///
    /// Returns `Some(CanonicalViewTx)` if the transaction exists in the canonical view,
    /// or `None` if the transaction doesn't exist or was excluded due to conflicts.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// # let txid = bitcoin::Txid::all_zeros();
    /// if let Some(canonical_tx) = view.tx(txid) {
    ///     println!("Found tx {} at position {:?}", canonical_tx.txid, canonical_tx.pos);
    /// }
    /// ```
    pub fn tx(&self, txid: Txid) -> Option<CanonicalTx<A>> {
        self.txs
            .get(&txid)
            .cloned()
            .map(|(tx, pos)| CanonicalTx { pos, txid, tx })
    }

    /// Get a single canonical transaction output.
    ///
    /// Returns detailed information about a transaction output, including whether it has been
    /// spent and by which transaction.
    ///
    /// Returns `None` if:
    /// - The transaction doesn't exist in the canonical view
    /// - The output index is out of bounds
    /// - The transaction was excluded due to conflicts
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::{OutPoint, hashes::Hash};
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// # let outpoint = OutPoint::default();
    /// if let Some(txout) = view.txout(outpoint) {
    ///     if txout.spent_by.is_some() {
    ///         println!("Output is spent");
    ///     } else {
    ///         println!("Output is unspent with value: {}", txout.txout.value);
    ///     }
    /// }
    /// ```
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

    /// Get an iterator over all canonical transactions in order.
    ///
    /// Transactions are returned in canonical order, with confirmed transactions ordered by
    /// block height and position, followed by unconfirmed transactions.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// // Iterate over all canonical transactions
    /// for tx in view.txs() {
    ///     println!("TX {}: {:?}", tx.txid, tx.pos);
    /// }
    ///
    /// // Get the total number of canonical transactions
    /// println!("Total canonical transactions: {}", view.txs().len());
    /// ```
    pub fn txs(&self) -> impl ExactSizeIterator<Item = CanonicalTx<A>> + DoubleEndedIterator + '_ {
        self.order.iter().map(|&txid| {
            let (tx, pos) = self.txs[&txid].clone();
            CanonicalTx { pos, txid, tx }
        })
    }

    /// Get a filtered list of outputs from the given outpoints.
    ///
    /// This method takes an iterator of `(identifier, outpoint)` pairs and returns an iterator
    /// of `(identifier, full_txout)` pairs for outpoints that exist in the canonical view.
    /// Non-existent outpoints are silently filtered out.
    ///
    /// The identifier type `O` can be any cloneable type and is passed through unchanged.
    /// This is useful for tracking which outpoints correspond to which addresses or keys.
    ///
    /// # Arguments
    ///
    /// * `outpoints` - An iterator of `(identifier, outpoint)` pairs to look up
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// // Get all outputs from an indexer
    /// for (keychain, txout) in view.filter_outpoints(indexer.outpoints().into_iter().map(|(k, op)| (k.clone(), *op))) {
    ///     println!("{}: {} sats", keychain.0, txout.txout.value);
    /// }
    /// ```
    pub fn filter_outpoints<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
    ) -> impl Iterator<Item = (O, FullTxOut<A>)> + 'v {
        outpoints
            .into_iter()
            .filter_map(|(op_i, op)| Some((op_i, self.txout(op)?)))
    }

    /// Get a filtered list of unspent outputs (UTXOs) from the given outpoints.
    ///
    /// Similar to [`filter_outpoints`](Self::filter_outpoints), but only returns outputs that
    /// have not been spent. This is useful for finding available UTXOs for spending.
    ///
    /// # Arguments
    ///
    /// * `outpoints` - An iterator of `(identifier, outpoint)` pairs to look up
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// // Get unspent outputs (UTXOs) from an indexer
    /// for (keychain, utxo) in view.filter_unspent_outpoints(indexer.outpoints().into_iter().map(|(k, op)| (k.clone(), *op))) {
    ///     println!("{} UTXO: {} sats", keychain.0, utxo.txout.value);
    /// }
    /// ```
    pub fn filter_unspent_outpoints<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
    ) -> impl Iterator<Item = (O, FullTxOut<A>)> + 'v {
        self.filter_outpoints(outpoints)
            .filter(|(_, txo)| txo.spent_by.is_none())
    }

    /// Calculate the total balance of the given outpoints.
    ///
    /// This method computes a detailed balance breakdown for a set of outpoints, categorizing
    /// outputs as confirmed, pending (trusted/untrusted), or immature based on their chain
    /// position and the provided trust predicate.
    ///
    /// # Arguments
    ///
    /// * `outpoints` - Iterator of `(identifier, outpoint)` pairs to calculate balance for
    /// * `trust_predicate` - Function that returns `true` for trusted scripts. Trusted outputs
    ///   count toward `trusted_pending` balance, while untrusted ones count toward
    ///   `untrusted_pending`
    /// * `min_confirmations` - Minimum confirmations required for an output to be considered
    ///   confirmed. Outputs with fewer confirmations are treated as pending.
    ///
    /// # Minimum Confirmations
    ///
    /// The `min_confirmations` parameter controls when outputs are considered confirmed:
    ///
    /// - `0` or `1`: Standard behavior - require at least 1 confirmation
    /// - `6`: Conservative - require 6 confirmations (often used for high-value transactions)
    /// - `100+`: May be used for coinbase outputs which require 100 confirmations
    ///
    /// Outputs with fewer than `min_confirmations` are categorized as pending (trusted or
    /// untrusted based on the trust predicate).
    ///
    /// # Balance Categories
    ///
    /// The returned [`Balance`] contains four categories:
    ///
    /// - `confirmed`: Outputs with ≥ `min_confirmations` and spendable
    /// - `trusted_pending`: Unconfirmed or insufficiently confirmed outputs from trusted scripts
    /// - `untrusted_pending`: Unconfirmed or insufficiently confirmed outputs from untrusted
    ///   scripts
    /// - `immature`: Coinbase outputs that haven't reached maturity (100 confirmations)
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// // Calculate balance with 6 confirmations, trusting all outputs
    /// let balance = view.balance(
    ///     indexer.outpoints().into_iter().map(|(k, op)| (k.clone(), *op)),
    ///     |_keychain, _script| true,  // Trust all outputs
    ///     6,  // Require 6 confirmations
    /// );
    ///
    /// // Or calculate balance trusting no outputs
    /// let untrusted_balance = view.balance(
    ///     indexer.outpoints().into_iter().map(|(k, op)| (k.clone(), *op)),
    ///     |_keychain, _script| false,  // Trust no outputs
    ///     1,
    /// );
    /// ```
    pub fn balance<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
        mut trust_predicate: impl FnMut(&O, ScriptBuf) -> bool,
        min_confirmations: u32,
    ) -> Balance {
        let mut immature = Amount::ZERO;
        let mut trusted_pending = Amount::ZERO;
        let mut untrusted_pending = Amount::ZERO;
        let mut confirmed = Amount::ZERO;

        for (spk_i, txout) in self.filter_unspent_outpoints(outpoints) {
            match &txout.chain_position {
                ChainPosition::Confirmed { anchor, .. } => {
                    let confirmation_height = anchor.confirmation_height_upper_bound();
                    let confirmations = self
                        .tip
                        .height
                        .saturating_sub(confirmation_height)
                        .saturating_add(1);
                    let min_confirmations = min_confirmations.max(1); // 0 and 1 behave identically

                    if confirmations < min_confirmations {
                        // Not enough confirmations, treat as trusted/untrusted pending
                        if trust_predicate(&spk_i, txout.txout.script_pubkey) {
                            trusted_pending += txout.txout.value;
                        } else {
                            untrusted_pending += txout.txout.value;
                        }
                    } else if txout.is_confirmed_and_spendable(self.tip.height) {
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

    /// List transaction IDs that are expected to exist for the given script pubkeys.
    ///
    /// This method is primarily used for synchronization with external sources, helping to
    /// identify which transactions are expected to exist for a set of script pubkeys. It's
    /// commonly used with
    /// [`SyncRequestBuilder::expected_spk_txids`](bdk_core::spk_client::SyncRequestBuilder::expected_spk_txids)
    /// to inform sync operations about known transactions.
    ///
    /// # Arguments
    ///
    /// * `indexer` - A script pubkey indexer (e.g., `KeychainTxOutIndex`) that tracks which scripts
    ///   are relevant
    /// * `spk_index_range` - A range bound to constrain which script indices to include. Use `..`
    ///   for all indices.
    ///
    /// # Returns
    ///
    /// An iterator of `(script_pubkey, txid)` pairs for all canonical transactions that involve
    /// the specified scripts.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalView, TxGraph, local_chain::LocalChain, spk_txout::SpkTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = CanonicalView::new(&tx_graph, &chain, chain.tip().block_id(), Default::default()).unwrap();
    /// # let indexer = SpkTxOutIndex::<u32>::default();
    /// // List all expected transactions for script indices 0-100
    /// for (script, txid) in view.list_expected_spk_txids(&indexer, 0..100) {
    ///     println!("Script {:?} appears in transaction {}", script, txid);
    /// }
    ///
    /// // List all expected transactions (no range constraint)
    /// for (script, txid) in view.list_expected_spk_txids(&indexer, ..) {
    ///     println!("Found transaction {} for script", txid);
    /// }
    /// ```
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
