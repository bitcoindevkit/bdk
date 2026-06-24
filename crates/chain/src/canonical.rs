//! Canonical view of transactions and unspent outputs.
//!
//! This module provides [`CanonicalView`], a utility for obtaining a canonical (ordered and
//! conflict-resolved) view of transactions from a [`TxGraph`].
//!
//! ## Example
//!
//! ```
//! # use bdk_chain::{TxGraph, CanonicalParams, CanonicalTask, local_chain::LocalChain};
//! # use bdk_core::BlockId;
//! # use bitcoin::hashes::Hash;
//! # let tx_graph = TxGraph::<BlockId>::default();
//! # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
//! let chain_tip = chain.tip().block_id();
//! let params = CanonicalParams::default();
//! let task = CanonicalTask::new(&tx_graph, chain_tip, params);
//! let view = chain.canonicalize(task);
//!
//! // Iterate over canonical transactions
//! for tx in view.txs() {
//!     println!("Transaction {}: {:?}", tx.txid, tx.pos);
//! }
//! ```

use crate::collections::{HashMap, HashSet};
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::{fmt, ops::RangeBounds};

use bdk_core::BlockId;
use bitcoin::{constants::COINBASE_MATURITY, OutPoint, ScriptBuf, Transaction, TxOut, Txid};

use crate::{spk_txout::SpkTxOutIndex, Anchor, Balance, CanonicalViewTask, ChainPosition, TxGraph};

/// A single canonical transaction with its position.
///
/// This struct represents a transaction that has been determined to be canonical (not
/// conflicted). It includes the transaction itself along with its position information.
/// The position type `P` is generic - it can be [`ChainPosition`] for resolved views,
/// or [`CanonicalReason`](crate::canonical_task::CanonicalReason) for unresolved canonicalization
/// results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalTx<P> {
    /// The position of this transaction.
    ///
    /// When `P` is [`ChainPosition`], this indicates whether the transaction is confirmed
    /// (and at what height) or unconfirmed (most likely pending in the mempool).
    pub pos: P,
    /// The transaction ID (hash) of this transaction.
    pub txid: Txid,
    /// The full transaction.
    pub tx: Arc<Transaction>,
}

impl<P: Ord> Ord for CanonicalTx<P> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.pos
            .cmp(&other.pos)
            // Txid tiebreaker for same position
            .then_with(|| self.txid.cmp(&other.txid))
    }
}

impl<P: Ord> PartialOrd for CanonicalTx<P> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A canonical transaction output with position and spend information.
///
/// The position type `P` is generic - it can be [`ChainPosition`] for resolved views,
/// or [`CanonicalReason`](crate::canonical_task::CanonicalReason) for unresolved canonicalization
/// results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalTxOut<P> {
    /// The position of the transaction in `outpoint` in the overall chain.
    pub pos: P,
    /// The location of the `TxOut`.
    pub outpoint: OutPoint,
    /// The `TxOut`.
    pub txout: TxOut,
    /// The txid and position of the transaction (if any) that has spent this output.
    pub spent_by: Option<(P, Txid)>,
    /// Whether this output is on a coinbase transaction.
    pub is_on_coinbase: bool,
}

impl<P: Ord> Ord for CanonicalTxOut<P> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.pos
            .cmp(&other.pos)
            // Tie-break with `outpoint` and `spent_by`.
            .then_with(|| self.outpoint.cmp(&other.outpoint))
            .then_with(|| self.spent_by.cmp(&other.spent_by))
    }
}

impl<P: Ord> PartialOrd for CanonicalTxOut<P> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Anchor> CanonicalTxOut<ChainPosition<A>> {
    /// Whether the `txout` is considered mature.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_mature(&self, tip: u32) -> bool {
        if self.is_on_coinbase {
            let conf_height = match self.pos.confirmation_height_upper_bound() {
                Some(height) => height,
                None => {
                    debug_assert!(false, "coinbase tx can never be unconfirmed");
                    return false;
                }
            };
            let age = tip.saturating_sub(conf_height);
            if age + 1 < COINBASE_MATURITY {
                return false;
            }
        }

        true
    }

    /// Whether the utxo is/was/will be spendable with chain `tip`.
    ///
    /// This method does not take into account the lock time.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_confirmed_and_spendable(&self, tip: u32) -> bool {
        if !self.is_mature(tip) {
            return false;
        }

        let conf_height = match self.pos.confirmation_height_upper_bound() {
            Some(height) => height,
            None => return false,
        };
        if conf_height > tip {
            return false;
        }

        // if the spending tx is confirmed within tip height, the txout is no longer spendable
        if let Some(spend_height) = self
            .spent_by
            .as_ref()
            .and_then(|(pos, _)| pos.confirmation_height_upper_bound())
        {
            if spend_height <= tip {
                return false;
            }
        }

        true
    }
}

/// Canonical set of transactions from a [`TxGraph`].
///
/// `Canonical` provides a conflict-resolved list of transactions. It determines
/// which transactions are canonical (non-conflicted) based on the current chain state and
/// provides methods to query transaction data, unspent outputs, and balances.
///
/// The position type `P` is generic:
/// - [`ChainPosition<A>`] for resolved views (aka [`CanonicalView`])
/// - [`CanonicalReason<A>`](crate::canonical_task::CanonicalReason) for unresolved results (aka
///   [`CanonicalTxs`])
///
/// The view maintains:
/// - A list of canonical transactions
/// - A mapping of outpoints to the transactions that spend them
/// - The chain tip used for canonicalization
///
/// [`TxGraph`]: crate::TxGraph
#[derive(Debug)]
pub struct Canonical<A, P> {
    /// List of canonical transaction IDs.
    pub(crate) order: Vec<Txid>,
    /// Map of transaction IDs to their transaction data and position.
    pub(crate) txs: HashMap<Txid, (Arc<Transaction>, P)>,
    /// Map of outpoints to the transaction ID that spends them.
    pub(crate) spends: HashMap<OutPoint, Txid>,
    /// The chain tip at the time this view was created.
    pub(crate) tip: BlockId,
    /// Marker for the anchor type.
    pub(crate) _anchor: core::marker::PhantomData<A>,
}

/// Type alias for canonical transactions with resolved [`ChainPosition`]s.
pub type CanonicalView<A> = Canonical<A, ChainPosition<A>>;

/// Type alias for canonical transactions with unresolved
/// [`CanonicalReason`](crate::canonical_task::CanonicalReason)s.
pub type CanonicalTxs<A> = Canonical<A, crate::canonical_task::CanonicalReason<A>>;

impl<A, P: Clone> Canonical<A, P> {
    /// Creates a [`Canonical`] from its constituent parts.
    ///
    /// This internal constructor is used by [`CanonicalTask`] to build the canonical set
    /// after completing the canonicalization process. It takes the processed transaction
    /// data including the canonical ordering, transaction map with positions, and
    /// spend information.
    pub(crate) fn new(
        tip: BlockId,
        order: Vec<Txid>,
        txs: HashMap<Txid, (Arc<Transaction>, P)>,
        spends: HashMap<OutPoint, Txid>,
    ) -> Self {
        Self {
            tip,
            order,
            txs,
            spends,
            _anchor: core::marker::PhantomData,
        }
    }

    /// Get the chain tip used to construct this canonical set.
    pub fn tip(&self) -> BlockId {
        self.tip
    }

    /// Get a single canonical transaction by its transaction ID.
    ///
    /// Returns `Some(CanonicalTx)` if the transaction exists in the canonical set,
    /// or `None` if the transaction doesn't exist or was excluded due to conflicts.
    pub fn tx(&self, txid: Txid) -> Option<CanonicalTx<P>> {
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
    /// - The transaction doesn't exist in the canonical set
    /// - The output index is out of bounds
    /// - The transaction was excluded due to conflicts
    pub fn txout(&self, op: OutPoint) -> Option<CanonicalTxOut<P>> {
        let (tx, pos) = self.txs.get(&op.txid)?;
        let vout: usize = op.vout.try_into().ok()?;
        let txout = tx.output.get(vout)?;
        let spent_by = self.spends.get(&op).map(|spent_by_txid| {
            let (_, spent_by_pos) = &self.txs[spent_by_txid];
            (spent_by_pos.clone(), *spent_by_txid)
        });
        Some(CanonicalTxOut {
            pos: pos.clone(),
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
    /// # use bdk_chain::{TxGraph, CanonicalTask, local_chain::LocalChain};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let task = CanonicalTask::new(&tx_graph, chain_tip, Default::default());
    /// # let view = chain.canonicalize(task);
    /// // Iterate over all canonical transactions
    /// for tx in view.txs() {
    ///     println!("TX {}: {:?}", tx.txid, tx.pos);
    /// }
    ///
    /// // Get the total number of canonical transactions
    /// println!("Total canonical transactions: {}", view.txs().len());
    /// ```
    pub fn txs(&self) -> impl ExactSizeIterator<Item = CanonicalTx<P>> + DoubleEndedIterator + '_ {
        self.order.iter().map(|&txid| {
            let (tx, pos) = self.txs[&txid].clone();
            CanonicalTx { pos, txid, tx }
        })
    }

    /// Get a filtered list of outputs from the given outpoints.
    ///
    /// This method takes an iterator of `(identifier, outpoint)` pairs and returns an iterator
    /// of `(identifier, canonical_txout)` pairs for outpoints that exist in the canonical set.
    /// Non-existent outpoints are silently filtered out.
    ///
    /// The identifier type `O` is useful for tracking which outpoints correspond to which addresses
    /// or keys.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{TxGraph, CanonicalTask, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let task = CanonicalTask::new(&tx_graph, chain_tip, Default::default());
    /// # let view = chain.canonicalize(task);
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// // Get all outputs from an indexer
    /// for (keychain, txout) in view.filter_outpoints(indexer.outpoints().clone()) {
    ///     println!("{}: {} sats", keychain.0, txout.txout.value);
    /// }
    /// ```
    pub fn filter_outpoints<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
    ) -> impl Iterator<Item = (O, CanonicalTxOut<P>)> + 'v {
        outpoints
            .into_iter()
            .filter_map(|(op_i, op)| Some((op_i, self.txout(op)?)))
    }

    /// Get a filtered list of unspent outputs (UTXOs) from the given outpoints.
    ///
    /// Similar to [`filter_outpoints`](Self::filter_outpoints), but only returns outputs that
    /// have not been spent. This is useful for finding available UTXOs for spending.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{TxGraph, CanonicalTask, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let task = CanonicalTask::new(&tx_graph, chain_tip, Default::default());
    /// # let view = chain.canonicalize(task);
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// // Get unspent outputs (UTXOs) from an indexer
    /// for (keychain, utxo) in view.filter_unspent_outpoints(indexer.outpoints().clone()) {
    ///     println!("{} UTXO: {} sats", keychain.0, utxo.txout.value);
    /// }
    /// ```
    pub fn filter_unspent_outpoints<'v, O: Clone + 'v>(
        &'v self,
        outpoints: impl IntoIterator<Item = (O, OutPoint)> + 'v,
    ) -> impl Iterator<Item = (O, CanonicalTxOut<P>)> + 'v {
        self.filter_outpoints(outpoints)
            .filter(|(_, txo)| txo.spent_by.is_none())
    }

    /// List transaction IDs that are expected to exist for the given script pubkeys.
    ///
    /// This method is primarily used for synchronization with external sources, helping to
    /// identify which transactions are expected to exist for a set of script pubkeys. It's
    /// commonly used with
    /// [`SyncRequestBuilder::expected_spk_txids`](bdk_core::spk_client::SyncRequestBuilder::expected_spk_txids)
    /// to inform sync operations about known transactions.
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

/// The spend-eligibility classification of a canonical output, produced by
/// [`CanonicalView::classify_outpoints`].
///
/// This is a *chain-level* classification: it captures only what the chain can determine
/// (settled-ness, maturity, and taint). It deliberately knows nothing about wallet policies such as
/// locked or reserved coins — callers layer their own categories on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Eligibility {
    /// A settled coinbase output that has not yet matured. Not spendable.
    Immature,
    /// Settled: the output's transaction is considered unlikely to be replaced.
    Settled,
    /// Pending (not settled), and neither it nor any of its unsettled ancestors taints it.
    TrustedPending,
    /// Pending (not settled), and it or one of its unsettled ancestors taints it.
    UntrustedPending,
}

impl<A: Anchor> CanonicalView<A> {
    /// Classify each of the given `outpoints` by its [spend eligibility](Eligibility).
    ///
    /// This is the primitive behind [`balance`](Self::balance) and is the building block for coin
    /// selection / coin control: it yields each unspent output paired with a chain-level
    /// [`Eligibility`], leaving aggregation (and any wallet-specific categories like "locked") to
    /// the caller. Spent outpoints, and outpoints not in this canonical view, are skipped.
    ///
    /// # Arguments
    ///
    /// * `outpoints` - Iterator of outpoints to classify.
    /// * `does_taint` - Function that returns `true` for transactions that should *taint* their
    ///   descendants. A pending output is [`UntrustedPending`](Eligibility::UntrustedPending) if
    ///   its transaction, or any of its unsettled ancestors, taints; otherwise
    ///   [`TrustedPending`](Eligibility::TrustedPending). See [Taint](#taint) below.
    /// * `is_settled` - Function that returns `true` for the [position](ChainPosition) of an output
    ///   whose transaction is considered settled — unlikely to be replaced (e.g. confirmed deeply
    ///   enough). Settled outputs are [`Settled`](Eligibility::Settled) or
    ///   [`Immature`](Eligibility::Immature); the rest are pending. `is_settled` is the sole
    ///   authority on this boundary — a settled output is never tainted. See [Settled](#settled)
    ///   below.
    ///
    /// # Settled
    ///
    /// `is_settled` controls the boundary between settled and pending outputs — i.e. whether we are
    /// confident a transaction will not be replaced. Typically it checks that an output has at
    /// least some number of confirmations, for example:
    ///
    /// ```
    /// # use bdk_chain::ChainPosition;
    /// # use bdk_core::BlockId;
    /// # let tip_height: u32 = 100;
    /// # let min_confirmations: u32 = 6;
    /// let is_settled = |pos: &ChainPosition<BlockId>| {
    ///     pos.confirmation_height_upper_bound()
    ///         .is_some_and(|h| tip_height.saturating_sub(h).saturating_add(1) >= min_confirmations)
    /// };
    /// # let _ = is_settled;
    /// ```
    ///
    /// # Taint
    ///
    /// `does_taint` decides whether a pending output is trusted. The canonical *unsettled* ancestry
    /// of each pending output is walked (stopping at settled transactions); if `does_taint` returns
    /// `true` for the output's own transaction or any walked ancestor, the output is
    /// [`UntrustedPending`](Eligibility::UntrustedPending), otherwise
    /// [`TrustedPending`](Eligibility::TrustedPending).
    ///
    /// A common use is to taint transactions that spend outputs the wallet does not own while
    /// unconfirmed (i.e. unconfirmed coins received from, or chained on top of, a third party).
    /// Returning `false` for every transaction treats all pending outputs as trusted; returning
    /// `true` treats them all as untrusted.
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalParams, ChainPosition, Eligibility, TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let view = chain.canonical_view(&tx_graph, chain.tip().block_id(), CanonicalParams::default());
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// // Coin control: prefer settled coins, fall back to trusted-pending; never spend the rest.
    /// let mut candidates = vec![];
    /// for (txout, eligibility) in view.classify_outpoints(
    ///     indexer.outpoints().iter().map(|(_, op)| *op),
    ///     |_tx| false, // Never taint
    ///     |pos: &ChainPosition<_>| pos.confirmation_height_upper_bound().is_some(),
    /// ) {
    ///     match eligibility {
    ///         Eligibility::Settled | Eligibility::TrustedPending => candidates.push(txout.outpoint),
    ///         Eligibility::Immature | Eligibility::UntrustedPending => {}
    ///     }
    /// }
    /// ```
    pub fn classify_outpoints(
        &self,
        outpoints: impl IntoIterator<Item = OutPoint>,
        mut does_taint: impl FnMut(CanonicalTx<ChainPosition<A>>) -> bool,
        is_settled: impl Fn(&ChainPosition<A>) -> bool,
    ) -> impl Iterator<Item = (CanonicalTxOut<ChainPosition<A>>, Eligibility)> {
        let utxos = outpoints
            .into_iter()
            .filter_map(|op| self.txout(op))
            .filter(|txo| txo.spent_by.is_none())
            .collect::<Vec<_>>();

        // The set of transaction ids of pending outputs that are tainted by themselves or an
        // unsettled ancestor.
        let tainted = {
            // Pending outputs seed the walk; settled ones cannot be tainted.
            let seeds = utxos
                .iter()
                .filter(|txo| !is_settled(&txo.pos))
                .map(|txo| txo.outpoint.txid)
                .collect::<HashSet<Txid>>();

            let mut tainted = HashSet::<Txid>::new();
            // Walk each pending output together with its unsettled ancestry (deduped across seeds,
            // stopping at settled transactions). Each transaction carries the set of descendants it
            // reaches; when an unsettled transaction taints, all of them are tainted. The settled
            // boundary is yielded but never taints (a settled transaction cannot be replaced).
            for (descendants, c_tx) in self.ancestors_inclusive::<BTreeSet<Txid>, _, _>(
                seeds.iter().copied(),
                |c_tx| core::iter::once(c_tx.txid).collect(),
                |c_tx| !is_settled(&c_tx.pos),
            ) {
                if !is_settled(&c_tx.pos) && does_taint(c_tx) {
                    tainted.extend(descendants);
                }
            }
            tainted
        };

        let tip = self.tip.height;
        utxos.into_iter().map(move |txout| {
            let eligibility = if is_settled(&txout.pos) {
                // Settled outputs are settled unless they are an immature coinbase. We rely on
                // `is_settled` alone (not on a confirmation height), so a caller is free to treat
                // unconfirmed outputs as settled.
                if txout.is_mature(tip) {
                    Eligibility::Settled
                } else {
                    Eligibility::Immature
                }
            } else if tainted.contains(&txout.outpoint.txid) {
                Eligibility::UntrustedPending
            } else {
                Eligibility::TrustedPending
            };
            (txout, eligibility)
        })
    }

    /// Calculate the total [`Balance`] of the given `outpoints`.
    ///
    /// This is a convenience fold over [`classify_outpoints`](Self::classify_outpoints): each
    /// output's value is added to the [`Balance`] bucket matching its [`Eligibility`]. See
    /// [`classify_outpoints`](Self::classify_outpoints) for the meaning of `does_taint` and
    /// `is_settled`, and for richer per-output handling (e.g. coin control).
    ///
    /// # Example
    ///
    /// ```
    /// # use bdk_chain::{CanonicalParams, ChainPosition, TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let view = chain.canonical_view(&tx_graph, chain_tip, CanonicalParams::default());
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// let tip_height = view.tip().height;
    /// // Calculate balance requiring 6 confirmations, tainting nothing (all pending trusted)
    /// let balance = view.balance(
    ///     indexer.outpoints().iter().map(|(_, op)| *op),
    ///     |_tx| false, // Never taint
    ///     |pos: &ChainPosition<_>| {
    ///         pos.confirmation_height_upper_bound()
    ///             .is_some_and(|h| tip_height.saturating_sub(h).saturating_add(1) >= 6)
    ///     },
    /// );
    /// ```
    pub fn balance(
        &self,
        outpoints: impl IntoIterator<Item = OutPoint>,
        does_taint: impl FnMut(CanonicalTx<ChainPosition<A>>) -> bool,
        is_settled: impl Fn(&ChainPosition<A>) -> bool,
    ) -> Balance {
        let mut balance = Balance::default();
        for (txout, eligibility) in self.classify_outpoints(outpoints, does_taint, is_settled) {
            let bucket = match eligibility {
                Eligibility::Immature => &mut balance.immature,
                Eligibility::Settled => &mut balance.settled,
                Eligibility::TrustedPending => &mut balance.trusted_pending,
                Eligibility::UntrustedPending => &mut balance.untrusted_pending,
            };
            *bucket += txout.txout.value;
        }
        balance
    }
}

impl<A: Anchor> CanonicalTxs<A> {
    /// Creates a [`CanonicalViewTask`] that resolves [`CanonicalReason`](crate::CanonicalReason)s
    /// into [`ChainPosition`]s.
    ///
    /// This is the second phase of the canonicalization pipeline. The resulting task
    /// queries the chain to verify anchors for transitively anchored transactions and
    /// produces a [`CanonicalView`] with resolved chain positions.
    pub fn view_task<'g>(self, tx_graph: &'g TxGraph<A>) -> CanonicalViewTask<'g, A> {
        CanonicalViewTask::new(tx_graph, self.tip, self.order, self.txs, self.spends)
    }
}
