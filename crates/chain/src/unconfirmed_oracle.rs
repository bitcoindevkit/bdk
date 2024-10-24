use core::convert::Infallible;

use bitcoin::Txid;

use crate::{collections::HashMap, Anchor, TxGraph};

/// Determines the canonical transaction from a set of conflicting unconfirmed transactions.
///
/// This is used for constructing the [`CanonicalView`].
pub trait UnconfirmedOracle {
    /// Error type.
    type Error;

    /// Unconfirmed position in the [`CanonicalView`].
    type UnconfirmedPos: Ord + Clone;

    /// Given a set of conflicting unconfirmed transactions, pick the transaction which is to be
    /// part of the canoncial history.
    ///
    /// If this returns `None`, it signals that none of the conflicts are to be considered
    /// canonical.
    fn pick_canonical<A, T>(
        &self,
        tx_graph: &TxGraph<A>,
        conflicting_txids: T,
    ) -> Result<Option<(Self::UnconfirmedPos, Txid)>, Self::Error>
    where
        A: Anchor,
        T: IntoIterator<Item = Txid>;
}

/// A simple [`UnconfirmedOracle`] implementation that uses `last_seen` in mempool values to
/// prioritize unconfirmed transactions.
#[derive(Debug, Clone, Copy, Default)]
pub struct LastSeenPrioritizer;

impl UnconfirmedOracle for LastSeenPrioritizer {
    type Error = Infallible;

    /// Last seen in mempool.
    type UnconfirmedPos = u64;

    fn pick_canonical<A, T>(
        &self,
        tx_graph: &TxGraph<A>,
        conflicting_txids: T,
    ) -> Result<Option<(Self::UnconfirmedPos, Txid)>, Self::Error>
    where
        A: Anchor,
        T: IntoIterator<Item = Txid>,
    {
        let mut best = Option::<(u64, Txid)>::None;
        for txid in conflicting_txids {
            let last_seen = tx_graph
                .get_tx_node(txid)
                .expect("must exist")
                .last_seen_unconfirmed;
            if let Some(last_seen) = last_seen {
                let this_key = (last_seen, txid);
                if Some(this_key) > best {
                    best = Some(this_key);
                }
            }
        }
        Ok(best)
    }
}

/// A [`UnconfirmedOracle`] implementation which allows setting a custom priority value per
/// transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomPrioritizer {
    prioritized_txids: HashMap<Txid, i32>,
}

/// Transaction to prioritize is missing from [`TxGraph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingTx(pub Txid);

impl core::fmt::Display for MissingTx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "missing transaction '{}'", self.0)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MissingTx {}
