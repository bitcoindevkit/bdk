//! The change set represents changes to [`TxGraph`](crate::TxGraph).

use alloc::sync::Arc;
use alloc::vec::Vec;

use bitcoin::{OutPoint, Transaction, TxOut, Txid};

use crate::collections::{BTreeMap, BTreeSet};
use crate::{Anchor, Merge};

/// The [`ChangeSet`] represents changes to a [`TxGraph`].
///
/// Since [`TxGraph`] is monotone, the "changeset" can only contain transactions to be added
/// and not removed.
///
/// Refer to the [module-level documentation](crate::tx_graph) for more.
///
/// [`TxGraph`]: crate::TxGraph
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: Ord + serde::Deserialize<'de>",
        serialize = "A: Ord + serde::Serialize",
    ))
)]
pub struct ChangeSet<A = ()> {
    /// Added transactions.
    pub txs: BTreeSet<Arc<Transaction>>,
    /// Added txouts.
    pub txouts: BTreeMap<OutPoint, TxOut>,
    /// Added anchors.
    pub anchors: BTreeSet<(A, Txid)>,
    /// Added last-seen unix timestamps of transactions.
    pub last_seen: BTreeMap<Txid, u64>,
}

impl<A> Default for ChangeSet<A> {
    fn default() -> Self {
        Self {
            txs: Default::default(),
            txouts: Default::default(),
            anchors: Default::default(),
            last_seen: Default::default(),
        }
    }
}

impl<A> ChangeSet<A> {
    /// Iterates over all outpoints contained within [`ChangeSet`].
    pub fn txouts(&self) -> impl Iterator<Item = (OutPoint, &TxOut)> {
        self.txs
            .iter()
            .flat_map(|tx| {
                tx.output
                    .iter()
                    .enumerate()
                    .map(move |(vout, txout)| (OutPoint::new(tx.compute_txid(), vout as _), txout))
            })
            .chain(self.txouts.iter().map(|(op, txout)| (*op, txout)))
    }

    /// Iterates over the heights of that the new transaction anchors in this changeset.
    ///
    /// This is useful if you want to find which heights you need to fetch data about in order to
    /// confirm or exclude these anchors.
    pub fn anchor_heights(&self) -> impl Iterator<Item = u32> + '_
    where
        A: Anchor,
    {
        let mut dedup = None;
        self.anchors
            .iter()
            .map(|(a, _)| a.anchor_block().height)
            .filter(move |height| {
                let duplicate = dedup == Some(*height);
                dedup = Some(*height);
                !duplicate
            })
    }
}

impl<A: Ord> ChangeSet<A> {
    /// Transform the [`ChangeSet`] to have [`Anchor`]s of another type.
    ///
    /// This takes in a closure of signature `FnMut(A) -> A2` which is called for each [`Anchor`] to
    /// transform it.
    pub fn map_anchors<A2: Ord, F>(self, mut f: F) -> ChangeSet<A2>
    where
        F: FnMut(A) -> A2,
    {
        ChangeSet {
            txs: self.txs,
            txouts: self.txouts,
            anchors: BTreeSet::<(A2, Txid)>::from_iter(
                self.anchors.into_iter().map(|(a, txid)| (f(a), txid)),
            ),
            last_seen: self.last_seen,
        }
    }
}

impl<A: Ord> Merge for ChangeSet<A> {
    fn merge(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        self.txs.extend(other.txs);
        self.txouts.extend(other.txouts);
        self.anchors.extend(other.anchors);

        // last_seen timestamps should only increase
        self.last_seen.extend(
            other
                .last_seen
                .into_iter()
                .filter(|(txid, update_ls)| self.last_seen.get(txid) < Some(update_ls))
                .collect::<Vec<_>>(),
        );
    }

    fn is_empty(&self) -> bool {
        self.txs.is_empty()
            && self.txouts.is_empty()
            && self.anchors.is_empty()
            && self.last_seen.is_empty()
    }
}

/// An indexed change set consists of a tx graph [`ChangeSet`] together with a
/// [`Indexer::ChangeSet`].
///
/// [`ChangeSet`]: super::ChangeSet
/// [`Indexer::ChangeSet`]: crate::Indexer::ChangeSet
pub mod indexed {
    use crate::tx_graph::changeset as tx;
    use crate::Merge;

    /// The [`ChangeSet`] represents changes to a [`TxGraph`] and [`Indexer`].
    ///
    /// Since [`TxGraph`] is monotone, the "changeset" can only contain transactions to be added
    /// and not removed.
    ///
    /// Refer to the [module-level documentation](crate::tx_graph) for more.
    ///
    /// [`TxGraph`]: crate::TxGraph
    /// [`Indexer`]: crate::Indexer
    #[derive(Debug, Clone, PartialEq)]
    #[cfg_attr(
        feature = "serde",
        derive(serde::Deserialize, serde::Serialize),
        serde(bound(
            deserialize = "A: Ord + serde::Deserialize<'de>, I: serde::Deserialize<'de>",
            serialize = "A: Ord + serde::Serialize, I: serde::Serialize",
        ))
    )]
    #[must_use]
    pub struct ChangeSet<A = (), I = ()> {
        /// Changes to tx graph
        pub tx_graph: tx::ChangeSet<A>,
        /// indexer changes
        pub indexer: I,
    }

    impl<A, I: Default> Default for ChangeSet<A, I> {
        fn default() -> Self {
            Self {
                tx_graph: Default::default(),
                indexer: Default::default(),
            }
        }
    }

    impl<A> From<tx::ChangeSet<A>> for ChangeSet<A> {
        fn from(tx_graph: tx::ChangeSet<A>) -> Self {
            Self {
                tx_graph,
                ..Default::default()
            }
        }
    }

    impl<A, I> From<I> for ChangeSet<A, I> {
        fn from(indexer: I) -> Self {
            Self {
                tx_graph: Default::default(),
                indexer,
            }
        }
    }

    impl<A: Ord, I: Merge> Merge for ChangeSet<A, I> {
        fn merge(&mut self, other: Self) {
            self.tx_graph.merge(other.tx_graph);
            self.indexer.merge(other.indexer);
        }

        fn is_empty(&self) -> bool {
            self.tx_graph.is_empty() && self.indexer.is_empty()
        }
    }
}
