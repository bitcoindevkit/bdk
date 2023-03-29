use alloc::collections::{BTreeMap, BTreeSet};
use bitcoin::{Block, BlockHash, OutPoint, Transaction, TxOut};

use crate::BlockId;

/// Trait to do something with every txout contained in a structure.
///
/// We would prefer to just work with things that can give us an `Iterator<Item=(OutPoint, &TxOut)>`
/// here, but rust's type system makes it extremely hard to do this (without trait objects).
pub trait ForEachTxOut {
    /// The provided closure `f` will be called with each `outpoint/txout` pair.
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut)));
}

impl ForEachTxOut for Block {
    fn for_each_txout(&self, mut f: impl FnMut((OutPoint, &TxOut))) {
        for tx in self.txdata.iter() {
            tx.for_each_txout(&mut f)
        }
    }
}

impl ForEachTxOut for Transaction {
    fn for_each_txout(&self, mut f: impl FnMut((OutPoint, &TxOut))) {
        let txid = self.txid();
        for (i, txout) in self.output.iter().enumerate() {
            f((
                OutPoint {
                    txid,
                    vout: i as u32,
                },
                txout,
            ))
        }
    }
}

/// Trait that "anchors" blockchain data in a specific block of height and hash.
///
/// This trait is typically associated with blockchain data such as transactions.
pub trait BlockAnchor:
    core::fmt::Debug + Clone + Eq + PartialOrd + Ord + core::hash::Hash + Send + Sync + 'static
{
    /// Returns the [`BlockId`] that the associated blockchain data is "anchored" in.
    fn anchor_block(&self) -> BlockId;
}

impl<A: BlockAnchor> BlockAnchor for &'static A {
    fn anchor_block(&self) -> BlockId {
        <A as BlockAnchor>::anchor_block(self)
    }
}

impl BlockAnchor for (u32, BlockHash) {
    fn anchor_block(&self) -> BlockId {
        (*self).into()
    }
}

/// Represents a service that tracks the best chain history.
/// TODO: How do we ensure the chain oracle is consistent across a single call?
/// * We need to somehow lock the data! What if the ChainOracle is remote?
/// * Get tip method! And check the tip still exists at the end! And every internal call
///   does not go beyond the initial tip.
pub trait ChainOracle {
    /// Error type.
    type Error: core::fmt::Debug;

    /// Returns the block hash (if any) of the given `height`.
    fn get_block_in_best_chain(&self, height: u32) -> Result<Option<BlockHash>, Self::Error>;

    /// Determines whether the block of [`BlockId`] exists in the best chain.
    fn is_block_in_best_chain(&self, block_id: BlockId) -> Result<bool, Self::Error> {
        Ok(matches!(self.get_block_in_best_chain(block_id.height)?, Some(h) if h == block_id.hash))
    }
}

// [TODO] We need stuff for smart pointers. Maybe? How does rust lib do this?
// Box<dyn ChainOracle>, Arc<dyn ChainOracle> ????? I will figure it out
impl<C: ChainOracle> ChainOracle for &C {
    type Error = C::Error;

    fn get_block_in_best_chain(&self, height: u32) -> Result<Option<BlockHash>, Self::Error> {
        <C as ChainOracle>::get_block_in_best_chain(self, height)
    }

    fn is_block_in_best_chain(&self, block_id: BlockId) -> Result<bool, Self::Error> {
        <C as ChainOracle>::is_block_in_best_chain(self, block_id)
    }
}

/// Represents changes to a [`TxIndex`] implementation.
pub trait TxIndexAdditions: Default {
    /// Append `other` on top of `self`.
    fn append_additions(&mut self, other: Self);
}

impl<I: Ord> TxIndexAdditions for BTreeSet<I> {
    fn append_additions(&mut self, mut other: Self) {
        self.append(&mut other);
    }
}

/// Represents an index of transaction data.
pub trait TxIndex {
    /// The resultant "additions" when new transaction data is indexed.
    type Additions: TxIndexAdditions;

    type SpkIndex: Ord;

    /// Scan and index the given `outpoint` and `txout`.
    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::Additions;

    /// Scan and index the given transaction.
    fn index_tx(&mut self, tx: &Transaction) -> Self::Additions {
        let txid = tx.txid();
        tx.output
            .iter()
            .enumerate()
            .map(|(vout, txout)| self.index_txout(OutPoint::new(txid, vout as _), txout))
            .reduce(|mut acc, other| {
                acc.append_additions(other);
                acc
            })
            .unwrap_or_default()
    }

    /// Apply additions to itself.
    fn apply_additions(&mut self, additions: Self::Additions);

    /// A transaction is relevant if it contains a txout with a script_pubkey that we own, or if it
    /// spends an already-indexed outpoint that we have previously indexed.
    fn is_tx_relevant(&self, tx: &Transaction) -> bool;

    /// Lists all relevant txouts known by the index.
    fn relevant_txouts(&self) -> &BTreeMap<OutPoint, (Self::SpkIndex, TxOut)>;
}
