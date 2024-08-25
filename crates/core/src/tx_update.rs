use crate::collections::{BTreeMap, BTreeSet, HashMap};
use alloc::{sync::Arc, vec::Vec};
use bitcoin::{OutPoint, Transaction, TxOut, Txid};

/// Data object used to communicate updates about relevant transactions from some chain data soruce
/// to the core model (usually a `bdk_chain::TxGraph`).
#[derive(Debug, Clone)]
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
    /// Extend this update with `other`.
    pub fn extend(&mut self, other: TxUpdate<A>) {
        self.txs.extend(other.txs);
        self.txouts.extend(other.txouts);
        self.anchors.extend(other.anchors);
        self.seen_ats.extend(other.seen_ats);
    }
}
