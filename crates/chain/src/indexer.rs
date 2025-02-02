//! [`Indexer`] provides utilities for indexing transaction data.

use bitcoin::{OutPoint, Transaction, TxOut};

#[cfg(feature = "miniscript")]
pub mod keychain_txout;
pub mod spk_txout;

/// Utilities for indexing transaction data.
///
/// An `Indexer` is the second type parameter of a [`TxGraph<A, X>`]. The `TxGraph` calls the
/// indexer whenever new transaction data is inserted into it, allowing the indexer to look at the
/// new data and mutate its state.
///
/// [`TxGraph<A, X>`]: crate::TxGraph
pub trait Indexer {
    /// The resultant "changeset" when new transaction data is indexed.
    type ChangeSet: Default + crate::Merge;

    /// Scan and index the given `outpoint` and `txout`.
    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::ChangeSet;

    /// Scans a transaction for relevant outpoints, which are stored and indexed internally.
    fn index_tx(&mut self, tx: &Transaction) -> Self::ChangeSet;

    /// Apply changeset to itself.
    fn apply_changeset(&mut self, changeset: Self::ChangeSet);

    /// Determines the [`ChangeSet`](Indexer::ChangeSet) between `self` and an empty [`Indexer`].
    fn initial_changeset(&self) -> Self::ChangeSet;

    /// Determines whether the transaction should be included in the index.
    fn is_tx_relevant(&self, tx: &Transaction) -> bool;
}

impl Indexer for () {
    type ChangeSet = ();

    fn index_txout(&mut self, _outpoint: OutPoint, _txout: &TxOut) -> Self::ChangeSet {}

    fn index_tx(&mut self, _tx: &Transaction) -> Self::ChangeSet {}

    fn apply_changeset(&mut self, _changeset: Self::ChangeSet) {}

    fn initial_changeset(&self) -> Self::ChangeSet {}

    fn is_tx_relevant(&self, _tx: &Transaction) -> bool {
        false
    }
}
