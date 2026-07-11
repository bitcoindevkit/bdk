use crate::collections::{BTreeMap, BTreeSet, HashSet};
use alloc::{sync::Arc, vec::Vec};
use bitcoin::{OutPoint, Transaction, TxOut, Txid};

/// Tracks fields already drained from a working [`TxUpdate`] via [`TxUpdate::drain_since`].
///
/// Create one cursor per sync/full_scan working buffer. Backends pass the same cursor to each
/// [`TxUpdate::drain_since`] call so partial events do not overlap. After all emissions, the
/// working buffer contains only undrained remainder (empty when fully streamed).
#[derive(Debug, Clone)]
pub struct TxUpdateCursor<A> {
    txs_len: usize,
    txouts: BTreeSet<OutPoint>,
    anchors: BTreeSet<(A, Txid)>,
    seen_ats: HashSet<(Txid, u64)>,
    evicted_ats: HashSet<(Txid, u64)>,
}

impl<A> TxUpdateCursor<A> {
    /// Create a cursor at the start of a new sync/full_scan working buffer.
    pub fn new() -> Self {
        Self {
            txs_len: 0,
            txouts: BTreeSet::new(),
            anchors: BTreeSet::new(),
            seen_ats: HashSet::new(),
            evicted_ats: HashSet::new(),
        }
    }
}

impl<A> Default for TxUpdateCursor<A> {
    fn default() -> Self {
        Self::new()
    }
}

/// Data object used to communicate updates about relevant transactions from some chain data source
/// to the core model (usually a `bdk_chain::TxGraph`).
///
/// ```rust
/// use bdk_core::TxUpdate;
/// # use std::sync::Arc;
/// # use bitcoin::{Transaction, transaction::Version, absolute::LockTime};
/// # let version = Version::ONE;
/// # let lock_time = LockTime::ZERO;
/// # let tx = Arc::new(Transaction { input: vec![], output: vec![], version, lock_time });
/// # let txid = tx.compute_txid();
/// # let anchor = ();
/// let mut tx_update = TxUpdate::default();
/// tx_update.txs.push(tx);
/// tx_update.anchors.insert((anchor, txid));
/// ```
/// ## Temporal context
/// To contribute to a wallet's balance, transactions must have an entry in either:
/// - [`Self::anchors`]: for confirmed transactions.
/// - [`Self::seen_ats`]: for unconfirmed transactions.
///
/// The built-in chain-source crates (`bdk_electrum`, `bdk_esplora`, `bdk_bitcoind_rpc`) handle this
/// automatically. Transactions lacking temporal context are stored but ignored by canonicalization.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TxUpdate<A = ()> {
    /// Full transactions. These are transactions that were determined to be relevant to the wallet
    /// given the request.
    pub txs: Vec<Arc<Transaction>>,

    /// Floating txouts. These are `TxOut`s that exist but the whole transaction wasn't included in
    /// `txs` since only knowing about the output is important. These are often used to help
    /// determine the fee of a wallet transaction.
    pub txouts: BTreeMap<OutPoint, TxOut>,

    /// Transaction anchors. Anchors tells us a position in the chain where a transaction was
    /// confirmed.
    pub anchors: BTreeSet<(A, Txid)>,

    /// When transactions were seen in the mempool.
    ///
    /// An unconfirmed transaction can only be canonical with a `seen_at` value. It is the
    /// responsibility of the chain-source to include the `seen_at` values for unconfirmed
    /// (unanchored) transactions.
    ///
    /// [`FullScanRequest::start_time`](crate::spk_client::FullScanRequest::start_time) or
    /// [`SyncRequest::start_time`](crate::spk_client::SyncRequest::start_time) can be used to
    /// provide the `seen_at` value.
    pub seen_ats: HashSet<(Txid, u64)>,

    /// When transactions were discovered to be missing (evicted) from the mempool.
    ///
    /// [`SyncRequest::start_time`](crate::spk_client::SyncRequest::start_time) can be used to
    /// provide the `evicted_at` value.
    pub evicted_ats: HashSet<(Txid, u64)>,
}

impl<A> Default for TxUpdate<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            txouts: Default::default(),
            anchors: Default::default(),
            seen_ats: Default::default(),
            evicted_ats: Default::default(),
        }
    }
}

impl<A> TxUpdate<A> {
    /// Returns true if the `TxUpdate` contains no elements in any of its fields.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
            && self.txouts.is_empty()
            && self.anchors.is_empty()
            && self.seen_ats.is_empty()
            && self.evicted_ats.is_empty()
    }
}

impl<A: Ord + Clone> TxUpdate<A> {
    /// Transforms the [`TxUpdate`] to have `anchors` (`A`) of another type (`A2`).
    ///
    /// This takes in a closure with signature `FnMut(A) -> A2` which is called for each anchor to
    /// transform it.
    pub fn map_anchors<A2: Ord, F: FnMut(A) -> A2>(self, mut map: F) -> TxUpdate<A2> {
        TxUpdate {
            txs: self.txs,
            txouts: self.txouts,
            anchors: self
                .anchors
                .into_iter()
                .map(|(a, txid)| (map(a), txid))
                .collect(),
            seen_ats: self.seen_ats,
            evicted_ats: self.evicted_ats,
        }
    }

    /// Extend this update with `other`.
    pub fn extend(&mut self, other: TxUpdate<A>) {
        self.txs.extend(other.txs);
        self.txouts.extend(other.txouts);
        self.anchors.extend(other.anchors);
        self.seen_ats.extend(other.seen_ats);
        self.evicted_ats.extend(other.evicted_ats);
    }

    /// Destructively drain newly accumulated data since the last call.
    ///
    /// Returns the delta and advances `cursor`. Whatever remains in `self` at sync end becomes
    /// `SyncResponse::tx_update` / `FullScanResponse::tx_update`. If everything was streamed via
    /// events, the remainder is empty and applying it at the end is a no-op.
    pub fn drain_since(&mut self, cursor: &mut TxUpdateCursor<A>) -> TxUpdate<A> {
        let mut delta = TxUpdate::default();

        if self.txs.len() > cursor.txs_len {
            delta.txs = self.txs.drain(cursor.txs_len..).collect();
            cursor.txs_len = self.txs.len();
        }

        let new_outpoints: Vec<OutPoint> = self
            .txouts
            .keys()
            .filter(|op| !cursor.txouts.contains(op))
            .cloned()
            .collect();
        for op in new_outpoints {
            if let Some(txout) = self.txouts.remove(&op) {
                delta.txouts.insert(op, txout);
                cursor.txouts.insert(op);
            }
        }

        let new_anchors: Vec<(A, Txid)> = self
            .anchors
            .iter()
            .filter(|entry| !cursor.anchors.contains(entry))
            .cloned()
            .collect();
        for entry in new_anchors {
            if self.anchors.remove(&entry) {
                delta.anchors.insert(entry.clone());
                cursor.anchors.insert(entry);
            }
        }

        let new_seen: Vec<(Txid, u64)> = self
            .seen_ats
            .iter()
            .filter(|entry| !cursor.seen_ats.contains(*entry))
            .cloned()
            .collect();
        for entry in new_seen {
            if self.seen_ats.remove(&entry) {
                delta.seen_ats.insert(entry);
                cursor.seen_ats.insert(entry);
            }
        }

        let new_evicted: Vec<(Txid, u64)> = self
            .evicted_ats
            .iter()
            .filter(|entry| !cursor.evicted_ats.contains(*entry))
            .cloned()
            .collect();
        for entry in new_evicted {
            if self.evicted_ats.remove(&entry) {
                delta.evicted_ats.insert(entry);
                cursor.evicted_ats.insert(entry);
            }
        }

        delta
    }
}
