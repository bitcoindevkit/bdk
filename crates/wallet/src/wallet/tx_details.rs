//! `TxDetails`

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use bitcoin::Txid;

use crate::collections::HashMap;

/// Kind of change that can be applied to [`TxDetails`]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Record {
    /// tx canceled
    Canceled,
    /// new details added. note, when applied this will overwrite existing
    Details(Details),
}

/// Communicates changes to [`TxDetails`]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    /// records
    pub records: Vec<(Txid, Record)>,
}

impl bdk_chain::Merge for ChangeSet {
    fn merge(&mut self, other: Self) {
        self.records.extend(other.records);
    }

    fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// The details of a wallet transaction
///
/// In the future this may be extended to include more metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    /// Cancellation status, `true` if the tx was canceled
    pub is_canceled: bool,
}

/// Object that stores transaction metadata
#[derive(Debug, Clone, Default)]
pub(crate) struct TxDetails {
    /// maps txid to details
    pub map: HashMap<Txid, Details>,
}

impl TxDetails {
    /// Marks a tx canceled
    pub fn cancel_tx(&mut self, txid: Txid) -> ChangeSet {
        let mut change = ChangeSet::default();

        let val = self.map.entry(txid).or_default();

        if val.is_canceled {
            return change;
        }

        val.is_canceled = true;

        change.records = vec![(txid, Record::Canceled)];

        change
    }

    /// Applies the given `changeset` to `self`
    pub fn apply_changeset(&mut self, changeset: ChangeSet) {
        for (txid, record) in changeset.records {
            match record {
                Record::Details(det) => {
                    let _ = self.map.insert(txid, det);
                }
                Record::Canceled => {
                    let _ = self.cancel_tx(txid);
                }
            }
        }
    }
}
