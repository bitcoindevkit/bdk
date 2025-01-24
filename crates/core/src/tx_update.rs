use crate::collections::{BTreeMap, BTreeSet, HashMap};
use alloc::{sync::Arc, vec::Vec};
use bitcoin::{OutPoint, Transaction, TxOut, Txid};

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
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TxUpdate<A = ()> {
    /// Full transactions. These are transactions that were determined to be relevant to the wallet
    /// given the request.
    pub txs: Vec<Arc<Transaction>>,
    /// Floating txouts. These are `TxOut`s that exist but the whole transaction wasn't included in
    /// `txs` since only knowing about the output is important. These are often used to help determine
    /// the fee of a wallet transaction.
    pub txouts: BTreeMap<OutPoint, TxOut>,
    /// Transaction anchors. Anchors tells us a position in the chain where a transaction was
    /// confirmed.
    pub anchors: BTreeSet<(A, Txid)>,
    /// Seen at times for transactions. This records when a transaction was most recently seen in
    /// the user's mempool for the sake of tie-breaking other conflicting transactions.
    pub seen_ats: HashMap<Txid, u64>,
}

impl<A> Default for TxUpdate<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            txouts: Default::default(),
            anchors: Default::default(),
            seen_ats: Default::default(),
        }
    }
}

impl<A: Ord> TxUpdate<A> {
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
        }
    }

    /// Extend this update with `other`.
    pub fn extend(&mut self, other: TxUpdate<A>) {
        self.txs.extend(other.txs);
        self.txouts.extend(other.txouts);
        self.anchors.extend(other.anchors);
        self.seen_ats.extend(other.seen_ats);
    }
}
