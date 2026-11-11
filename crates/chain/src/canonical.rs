//! Canonical view of transactions and unspent outputs.
//!
//! This module provides [`CanonicalView`], a utility for obtaining a canonical (ordered and
//! conflict-resolved) view of transactions from a [`TxGraph`].
//!
//! ## Example
//!
//! ```
//! # use bdk_chain::{TxGraph, CanonicalParams, local_chain::LocalChain};
//! # use bdk_core::BlockId;
//! # use bitcoin::hashes::Hash;
//! # let tx_graph = TxGraph::<BlockId>::default();
//! # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
//! let chain_tip = chain.tip().block_id();
//! let params = CanonicalParams::default();
//! let view = chain.canonicalize(&tx_graph, chain_tip, params);
//!
//! // Iterate over canonical transactions
//! for tx in view.txs() {
//!     println!("Transaction {}: {:?}", tx.txid, tx.pos);
//! }
//! ```

use crate::collections::{HashMap, HashSet};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::{fmt, ops::RangeBounds};

use bdk_core::{BlockId, BlockQueries};
use bitcoin::{
    absolute, constants::COINBASE_MATURITY, relative, OutPoint, ScriptBuf, Transaction, TxOut, Txid,
};

use crate::{spk_txout::SpkTxOutIndex, Anchor, Balance, CanonicalViewTask, ChainPosition, TxGraph};

/// The spend-eligibility classification of a canonical output, produced by
/// [`CanonicalView::classify_outpoints`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Eligibility {
    /// An output the caller considers settled, per the `is_settled` predicate given to
    /// [`classify_outpoints`](CanonicalView::classify_outpoints). Typically confirmed deeply
    /// enough to be unlikely to be replaced, but the caller decides.
    Settled,
    /// An output not yet spendable by consensus: an immature coinbase output, or one whose
    /// [`Timelocks`] are not met. May be unconfirmed; its [`Trust`] is not evaluated.
    Immature,
    /// An output not yet settled.
    Unsettled(Trust),
}

/// The timelocks of the spend path the caller will use for an output. A path can have both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Timelocks {
    /// The `nLockTime` the spending transaction needs (BIP-65, e.g. miniscript `after`).
    pub absolute: Option<absolute::LockTime>,
    /// The `nSequence` the spending input needs (BIP-112, e.g. miniscript `older`).
    pub relative: Option<relative::LockTime>,
}

/// Describes whether an [`Unsettled`](Eligibility::Unsettled) output is trusted, untrusted, or of
/// unknown trust because the `CanonicalView` doesn't have its full ancestry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trust {
    /// Ancestors spend owned outputs.
    Trusted,
    /// Ancestors spend foreign outputs.
    Untrusted,
    /// Some ancestor is not in Canonical set.
    Unknown,
}

/// Internal per-transaction entry in [`Canonical`].
#[derive(Clone, Debug)]
pub(crate) struct CanonicalEntry<P> {
    pub(crate) tx: Arc<Transaction>,
    pub(crate) pos: P,
    pub(crate) prev_mtp: Option<u32>,
}

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
    /// The median-time-past of the block *preceding* the confirmation height, if computed.
    ///
    /// This is the MTP at `confirmation_height - 1` (saturating at genesis), which is the
    /// reference value used to evaluate BIP-68 relative timelocks on spends of this output,
    /// matching Bitcoin Core's `CalculateSequenceLocks`. It is *not* the MTP at the
    /// confirmation height itself.
    ///
    /// `None` means the value is unknown, which happens whenever any of these hold:
    ///
    /// - MTP computation was not enabled; see
    ///   [`CanonicalViewTask::with_mtp`](crate::CanonicalViewTask::with_mtp).
    /// - The output's transaction is not confirmed.
    /// - The output's transaction is confirmed only *transitively*, through a descendant's anchor,
    ///   so its own confirmation height genuinely isn't known.
    /// - The chain could not resolve the full 11-block window the MTP medians over.
    ///
    /// So a confirmed output can have `None` here even with MTP enabled. Do not read `None` as
    /// "MTP was off", and never treat it as an absent or satisfied timelock.
    pub prev_mtp: Option<u32>,
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
    /// A coinbase output is mature once a transaction spending it can be included in the block
    /// after `tip`. Consensus requires the spending block to be at least [`COINBASE_MATURITY`]
    /// blocks above the confirming block, so the output is mature once `tip` is at least
    /// `COINBASE_MATURITY - 1` blocks above the confirming block. Non-coinbase outputs are always
    /// mature.
    ///
    /// If the position is confirmed [`transitively`](ChainPosition::Confirmed::transitively), the
    /// confirmation height may be an upper bound (see [`confirmation_height_upper_bound`]), so
    /// this method may return false-negatives.
    ///
    /// [`confirmation_height_upper_bound`]: ChainPosition::confirmation_height_upper_bound
    pub fn is_mature(&self, tip: u32) -> bool {
        if self.is_on_coinbase {
            let conf_height = match self.pos.confirmation_height_upper_bound() {
                Some(height) => height,
                // A coinbase can be canonical without us holding an anchor for it (i.e. it is an
                // ancestor of a canonical tx, or is assumed canonical). With no known
                // confirmation height, maturity cannot be proven.
                None => return false,
            };
            let age = tip.saturating_sub(conf_height);
            if age + 1 < COINBASE_MATURITY {
                return false;
            }
        }

        true
    }

    /// Whether `timelocks` are met for a transaction spending this output in the block after
    /// `tip`, where `tip_mtp` is the MTP at `tip` (see [`Canonical::tip_mtp`]).
    ///
    /// This follows consensus (BIP-65, BIP-68, BIP-113). An unconfirmed output is treated as
    /// confirming in the block after `tip`, as Bitcoin Core does for mempool coins. A lock that
    /// needs an unknown MTP (`None` [`prev_mtp`](Self::prev_mtp) or `tip_mtp`) counts as unmet.
    ///
    /// If the position is confirmed [`transitively`](ChainPosition::Confirmed::transitively), the
    /// confirmation height may be an upper bound (see [`confirmation_height_upper_bound`]), so
    /// this method may return false-negatives.
    ///
    /// [`confirmation_height_upper_bound`]: ChainPosition::confirmation_height_upper_bound
    pub fn are_timelocks_met(&self, timelocks: Timelocks, tip: u32, tip_mtp: Option<u32>) -> bool {
        let next_height = tip.saturating_add(1);
        let absolute_met = match timelocks.absolute {
            None => true,
            Some(absolute::LockTime::Blocks(h)) => h.to_consensus_u32() < next_height,
            Some(absolute::LockTime::Seconds(t)) => {
                tip_mtp.is_some_and(|mtp| t.to_consensus_u32() < mtp)
            }
        };
        let relative_met = match timelocks.relative {
            None => true,
            Some(relative::LockTime::Blocks(n)) => {
                let conf_height = self
                    .pos
                    .confirmation_height_upper_bound()
                    .unwrap_or(next_height);
                conf_height.saturating_add(n.value().into()) <= next_height
            }
            Some(relative::LockTime::Time(n)) => {
                let coin_mtp = if self.pos.is_confirmed() {
                    self.prev_mtp
                } else {
                    tip_mtp
                };
                match (coin_mtp, tip_mtp) {
                    (Some(coin_mtp), Some(tip_mtp)) => {
                        u64::from(coin_mtp) + u64::from(n.value()) * 512 <= u64::from(tip_mtp)
                    }
                    _ => false,
                }
            }
        };
        absolute_met && relative_met
    }

    /// Whether the utxo is/was/will be spendable with chain `tip`.
    ///
    /// This method does not take into account the lock time.
    ///
    /// If the position is confirmed [`transitively`](ChainPosition::Confirmed::transitively), the
    /// confirmation height may be an upper bound (see [`confirmation_height_upper_bound`]), so
    /// this method may return false-negatives.
    ///
    /// [`confirmation_height_upper_bound`]: ChainPosition::confirmation_height_upper_bound
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
    /// Map of transaction IDs to their transaction data, position, and MTP.
    pub(crate) txs: HashMap<Txid, CanonicalEntry<P>>,
    /// Map of outpoints to the transaction ID that spends them.
    pub(crate) spends: HashMap<OutPoint, Txid>,
    /// The chain tip at the time this view was created.
    pub(crate) tip: BlockId,
    /// Median-time-past at the chain tip height.
    pub(crate) tip_mtp: Option<u32>,
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
        txs: HashMap<Txid, CanonicalEntry<P>>,
        spends: HashMap<OutPoint, Txid>,
        tip_mtp: Option<u32>,
    ) -> Self {
        Self {
            tip,
            order,
            txs,
            spends,
            tip_mtp,
            _anchor: core::marker::PhantomData,
        }
    }

    /// Get the chain tip used to construct this canonical set.
    pub fn tip(&self) -> BlockId {
        self.tip
    }

    /// Get the MTP at the chain tip height.
    ///
    /// Returns `None` if MTP was not computed.
    pub fn tip_mtp(&self) -> Option<u32> {
        self.tip_mtp
    }

    /// Get a single canonical transaction by its transaction ID.
    ///
    /// Returns `Some(CanonicalTx)` if the transaction exists in the canonical set,
    /// or `None` if the transaction doesn't exist or was excluded due to conflicts.
    pub fn tx(&self, txid: Txid) -> Option<CanonicalTx<P>> {
        let entry = self.txs.get(&txid)?;
        Some(CanonicalTx {
            pos: entry.pos.clone(),
            txid,
            tx: entry.tx.clone(),
        })
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
        let entry = self.txs.get(&op.txid)?;
        let vout: usize = op.vout.try_into().ok()?;
        let txout = entry.tx.output.get(vout)?;
        let spent_by = self.spends.get(&op).map(|spent_by_txid| {
            let spent_by_entry = &self.txs[spent_by_txid];
            (spent_by_entry.pos.clone(), *spent_by_txid)
        });
        Some(CanonicalTxOut {
            pos: entry.pos.clone(),
            outpoint: op,
            txout: txout.clone(),
            spent_by,
            is_on_coinbase: entry.tx.is_coinbase(),
            prev_mtp: entry.prev_mtp,
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
    /// # use bdk_chain::{TxGraph, local_chain::LocalChain};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let view = chain.canonicalize(&tx_graph, chain_tip, Default::default());
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
            let entry = &self.txs[&txid];
            CanonicalTx {
                pos: entry.pos.clone(),
                txid,
                tx: entry.tx.clone(),
            }
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
    /// # use bdk_chain::{TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let view = chain.canonicalize(&tx_graph, chain_tip, Default::default());
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
    /// # use bdk_chain::{TxGraph, local_chain::LocalChain, keychain_txout::KeychainTxOutIndex};
    /// # use bdk_core::BlockId;
    /// # use bitcoin::hashes::Hash;
    /// # let tx_graph = TxGraph::<BlockId>::default();
    /// # let chain = LocalChain::from_blocks([(0, bitcoin::BlockHash::all_zeros())].into_iter().collect()).unwrap();
    /// # let chain_tip = chain.tip().block_id();
    /// # let view = chain.canonicalize(&tx_graph, chain_tip, Default::default());
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

impl<A: Anchor> CanonicalView<A> {
    /// Classify each of the given `outpoints` by its [spend eligibility](Eligibility).
    /// This is the primitive behind [`balance`](Self::balance).
    ///  
    /// Callers that need richer handling (coin selection, coin control, or
    /// wallet-specific categories) can fold over this instead of `balance`.
    ///
    /// Outpoints that are already spent, or that aren't part of this canonical view, are skipped.
    ///
    /// # Arguments
    ///
    /// * `outpoints` - The outpoints to classify.
    /// * `does_taint` - Returns `true` for a transaction that pulls in untrusted funds (e.g. it
    ///   spends an output the wallet doesn't own). It drives the [`Trust`] of an unsettled output:
    ///   a tainting transaction in its ancestry makes it [`Untrusted`](Trust::Untrusted). Outputs
    ///   with missing ancestry stay [`Unknown`](Trust::Unknown) regardless of this predicate. Use
    ///   [`taints_unowned`] to classify everything foreign as untrusted.
    /// * `is_settled` - Returns `true` for the [position](ChainPosition) of a transaction we
    ///   consider settled (unlikely to be replaced), for example one with enough confirmations.
    /// * `timelocks` - Returns the [`Timelocks`] of the spend path the caller will use for a txout.
    ///   A txout whose timelocks are not met (see
    ///   [`are_timelocks_met`](CanonicalTxOut::are_timelocks_met)) is
    ///   [`Immature`](Eligibility::Immature). Time-based locks need MTP, so on a view without it
    ///   (see [`CanonicalViewTask::with_mtp`]) they are never met.
    pub fn classify_outpoints<'a>(
        &'a self,
        outpoints: impl IntoIterator<Item = OutPoint> + 'a,
        mut does_taint: impl FnMut(&CanonicalTx<ChainPosition<A>>) -> bool + 'a,
        is_settled: impl Fn(&ChainPosition<A>) -> bool + 'a,
        timelocks: impl Fn(&CanonicalTxOut<ChainPosition<A>>) -> Timelocks + 'a,
    ) -> impl Iterator<Item = (CanonicalTxOut<ChainPosition<A>>, Eligibility)> + 'a {
        let tip = self.tip.height;
        let tip_mtp = self.tip_mtp;
        // Shared across outpoints so an ancestor reached by several of them is only walked once.
        let mut cache = HashMap::<Txid, Trust>::new();
        outpoints
            .into_iter()
            .filter_map(move |op| self.txout(op))
            .filter(|txo| txo.spent_by.is_none())
            .map(move |txout| {
                let eligibility = if !txout.is_mature(tip)
                    || !txout.are_timelocks_met(timelocks(&txout), tip, tip_mtp)
                {
                    Eligibility::Immature
                } else if is_settled(&txout.pos) {
                    Eligibility::Settled
                } else {
                    Eligibility::Unsettled(self.ancestry_trust(
                        txout.outpoint.txid,
                        &mut does_taint,
                        &is_settled,
                        &mut cache,
                    ))
                };
                (txout, eligibility)
            })
    }

    /// Returns the [`Trust`] of `seed_txid` based on its unsettled ancestry.
    ///
    /// Walks backwards from `seed_txid`, stopping at settled ancestors.
    /// An ancestor missing from the [`CanonicalView`] set is [`Unknown`](Trust::Unknown).
    /// Each visited transaction is cached, so an ancestor shared by several outpoints only gets
    /// walked once across the calls to this method that share the same `cache`.
    /// The walk stops at [`Settled`](Eligibility::Settled) transactions and at
    /// [`Unknown`](Trust::Unknown) ones, and performs a short-circuit as soon as a tainting
    /// transaction is found.
    fn ancestry_trust<F, S>(
        &self,
        seed_txid: Txid,
        does_taint: &mut F,
        is_settled: &S,
        cache: &mut HashMap<Txid, Trust>,
    ) -> Trust
    where
        F: FnMut(&CanonicalTx<ChainPosition<A>>) -> bool,
        S: Fn(&ChainPosition<A>) -> bool,
    {
        if let Some(&trust) = cache.get(&seed_txid) {
            return trust;
        }

        // `Enter`: if tx is unsettled and not directly tainted, queue its parents.
        // `Exit`: by now every parent is resolved, so the tx is tainted if any parent is.
        enum Frame<A> {
            Enter(Txid),
            Exit(CanonicalTx<ChainPosition<A>>),
        }

        let mut stack = alloc::vec![Frame::Enter(seed_txid)];
        // Txids currently on the stack between their `Enter` and `Exit`, so we don't queue the
        // same parent twice while it's still being processed.
        let mut pending = HashSet::<Txid>::new();

        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Enter(txid) => {
                    if cache.contains_key(&txid) || pending.contains(&txid) {
                        continue;
                    }
                    let Some(c_tx) = self.tx(txid) else {
                        // Missing from the `CanonicalView`.
                        cache.insert(txid, Trust::Unknown);
                        continue;
                    };
                    if is_settled(&c_tx.pos) {
                        cache.insert(txid, Trust::Trusted);
                        continue;
                    }
                    if does_taint(&c_tx) {
                        // Directly tainted
                        cache.insert(txid, Trust::Untrusted);
                        continue;
                    }
                    pending.insert(txid);
                    stack.push(Frame::Exit(c_tx.clone()));
                    for txin in &c_tx.tx.input {
                        // Previous output is coinbase
                        if txin.previous_output.is_null() {
                            continue;
                        }
                        let parent_txid = txin.previous_output.txid;
                        if !cache.contains_key(&parent_txid) && !pending.contains(&parent_txid) {
                            stack.push(Frame::Enter(parent_txid));
                        }
                    }
                }
                Frame::Exit(c_tx) => {
                    let mut trust = Trust::Trusted;
                    for txin in &c_tx.tx.input {
                        if txin.previous_output.is_null() {
                            continue;
                        }
                        let parent_trust = cache
                            .get(&txin.previous_output.txid)
                            .copied()
                            .expect("parent transaction should already be cached");
                        trust = match (trust, parent_trust) {
                            (Trust::Untrusted, _) | (_, Trust::Untrusted) => Trust::Untrusted,
                            (Trust::Unknown, _) | (_, Trust::Unknown) => Trust::Unknown,
                            _ => Trust::Trusted,
                        };
                        if trust == Trust::Untrusted {
                            break;
                        }
                    }
                    cache.insert(c_tx.txid, trust);
                    pending.remove(&c_tx.txid);
                }
            }
        }

        cache[&seed_txid]
    }

    /// Calculate the total [`Balance`] of the given outpoints.
    ///
    /// This is a fold over [`classify_outpoints`](Self::classify_outpoints): each output's value is
    /// added to the bucket matching its [`Eligibility`].
    ///
    /// See `classify_outpoints` for `does_taint`, `is_settled` and `timelocks` meaning.
    ///
    /// The position of an output confirmed [`transitively`](ChainPosition::Confirmed::transitively)
    /// carries a descendant's anchor, so its confirmation height may be an upper bound (see
    /// [`confirmation_height_upper_bound`]). An `is_settled` that requires a minimum number of
    /// confirmations may therefore count such an output as pending even though it actually has
    /// enough.
    ///
    /// [`confirmation_height_upper_bound`]: ChainPosition::confirmation_height_upper_bound
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
    /// # let view = chain.canonicalize(&tx_graph, chain_tip, CanonicalParams::default());
    /// # let indexer = KeychainTxOutIndex::<&str>::default();
    /// let tip_height = view.tip().height;
    /// // Calculate balance requiring 6 confirmations.
    /// let balance = view.balance(
    ///     indexer.outpoints().iter().map(|(_, op)| *op),
    ///     bdk_chain::taints_unowned(&indexer),
    ///     |pos: &ChainPosition<_>| {
    ///         pos.confirmation_height_upper_bound()
    ///             .is_some_and(|h| tip_height.saturating_sub(h).saturating_add(1) >= 6)
    ///     },
    ///     |_txout| Default::default(),
    /// );
    /// ```
    pub fn balance(
        &self,
        outpoints: impl IntoIterator<Item = OutPoint>,
        does_taint: impl FnMut(&CanonicalTx<ChainPosition<A>>) -> bool,
        is_settled: impl Fn(&ChainPosition<A>) -> bool,
        timelocks: impl Fn(&CanonicalTxOut<ChainPosition<A>>) -> Timelocks,
    ) -> Balance {
        self.classify_outpoints(outpoints, does_taint, is_settled, timelocks)
            .collect()
    }
}

impl<A: Anchor> FromIterator<(CanonicalTxOut<ChainPosition<A>>, Eligibility)> for Balance {
    /// Sums each output's value into the [`Balance`] bucket matching its [`Eligibility`].
    fn from_iter<I: IntoIterator<Item = (CanonicalTxOut<ChainPosition<A>>, Eligibility)>>(
        iter: I,
    ) -> Self {
        let mut balance = Balance::default();
        for (txout, eligibility) in iter {
            let bucket = match eligibility {
                Eligibility::Immature => &mut balance.immature,
                Eligibility::Settled => &mut balance.confirmed,
                Eligibility::Unsettled(Trust::Trusted) => &mut balance.trusted_pending,
                Eligibility::Unsettled(Trust::Untrusted | Trust::Unknown) => {
                    &mut balance.untrusted_pending
                }
            };
            *bucket += txout.txout.value;
        }
        balance
    }
}

/// A `does_taint` predicate: a transaction taints when any input spends a coin this indexer
/// doesn't recognise as ours, including one whose parent we've never seen.
pub fn taints_unowned<'a, P, I>(
    indexer: &'a impl AsRef<SpkTxOutIndex<I>>,
) -> impl Fn(&CanonicalTx<P>) -> bool + 'a
where
    I: fmt::Debug + Clone + Ord + 'a,
{
    let indexer = indexer.as_ref();
    move |c_tx| {
        c_tx.tx.input.iter().any(|txin| {
            !txin.previous_output.is_null() && indexer.txout(txin.previous_output).is_none()
        })
    }
}

impl<A: Anchor> CanonicalTxs<A> {
    /// Creates a [`CanonicalViewTask`] that resolves
    /// [`CanonicalReason`](crate::canonical_task::CanonicalReason)s into [`ChainPosition`]s.
    ///
    /// This is the second phase of the canonicalization pipeline. Blocks resolved during
    /// phase 1 are reused without redundant queries; requests still outstanding in `queries` are
    /// dropped and made again if needed. If `queries` is for a different tip than this set, its
    /// blocks are not reused at all.
    ///
    /// To also compute median-time-past (MTP) values for the resulting view, call
    /// [`CanonicalViewTask::with_mtp`] on the returned task.
    pub fn view_task<'g, B>(
        self,
        tx_graph: &'g TxGraph<A>,
        queries: BlockQueries<B>,
    ) -> CanonicalViewTask<'g, A, B> {
        // Blocks fetched for another tip may not be in this tip's chain.
        let queries = if queries.tip() == self.tip {
            queries
        } else {
            BlockQueries::new(self.tip)
        };
        CanonicalViewTask::new(tx_graph, self.order, self.txs, self.spends, queries)
    }
}
