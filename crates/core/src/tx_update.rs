use crate::collections::{BTreeMap, BTreeSet, HashSet};
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
/// ## Temporal context
/// To contribute to a wallet's balance, transactions must have an entry in either:
/// - [`Self::anchors`]: for confirmed transactions.
/// - [`Self::seen_ats`]: for unconfirmed transactions.
///
/// The built-in chain-source crates (`bdk_electrum`, `bdk_esplora`, `bdk_bitcoind_rpc`) handle this
/// automatically. Transactions lacking temporal context are stored but ignored by canonicalization.
#[derive(Debug, Clone)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(bound(
        serialize = "A: serde::Serialize",
        deserialize = "A: Ord + serde::Deserialize<'de>"
    ))
)]
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
}

#[cfg(all(test, feature = "serde"))]
mod test {
    use super::*;
    use crate::{BlockId, ConfirmationBlockTime};
    use bitcoin::{hashes::Hash, Amount, ScriptBuf};

    #[test]
    fn tx_update_serde_round_trip() {
        let tx = Transaction {
            version: bitcoin::transaction::Version::non_standard(0),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        };
        let txid = tx.compute_txid();

        let mut tx_update = TxUpdate::<ConfirmationBlockTime>::default();
        tx_update.txs.push(Arc::new(tx));
        tx_update.txouts.insert(
            OutPoint::new(txid, 0),
            TxOut {
                value: Amount::from_sat(42_000),
                script_pubkey: ScriptBuf::new(),
            },
        );
        tx_update.anchors.insert((
            ConfirmationBlockTime {
                block_id: BlockId {
                    height: 7,
                    hash: Hash::hash(b"anchor"),
                },
                confirmation_time: 1_700_000_000,
            },
            txid,
        ));
        tx_update.seen_ats.insert((txid, 1_700_000_001));
        tx_update.evicted_ats.insert((txid, 1_700_000_002));

        let json = serde_json::to_string(&tx_update).expect("serialization must succeed");
        let restored: TxUpdate<ConfirmationBlockTime> =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(tx_update.txs, restored.txs);
        assert_eq!(tx_update.txouts, restored.txouts);
        assert_eq!(tx_update.anchors, restored.anchors);
        assert_eq!(tx_update.seen_ats, restored.seen_ats);
        assert_eq!(tx_update.evicted_ats, restored.evicted_ats);
    }
}
