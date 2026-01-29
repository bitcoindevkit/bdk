//! [`Indexer`] provides utilities for indexing transaction data.

use bitcoin::{OutPoint, Transaction, TxOut};

#[cfg(feature = "miniscript")]
pub mod keychain_txout;
pub mod spk_txout;

/// Utilities for indexing transaction data.
///
/// Types which implement this trait can be used to construct an [`IndexedTxGraph`].
/// This trait's methods should rarely be called directly.
///
/// [`IndexedTxGraph`]: crate::IndexedTxGraph
pub trait Indexer {
    /// The resultant "changeset" when new transaction data is indexed.
    type ChangeSet;

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
