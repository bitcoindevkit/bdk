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

/// Trait that "anchors" blockchain data to a specific block of height and hash.
///
/// I.e. If transaction A is anchored in block B, then if block B is in the best chain, we can
/// assume that transaction A is also confirmed in the best chain. This does not necessarily mean
/// that transaction A is confirmed in block B. It could also mean transaction A is confirmed in a
/// parent block of B.
pub trait Anchor:
    core::fmt::Debug + Clone + Eq + PartialOrd + Ord + core::hash::Hash + Send + Sync + 'static
{
    /// Returns the [`BlockId`] that the associated blockchain data is "anchored" in.
    fn anchor_block(&self) -> BlockId;
}

impl<A: Anchor> Anchor for &'static A {
    fn anchor_block(&self) -> BlockId {
        <A as Anchor>::anchor_block(self)
    }
}

impl Anchor for (u32, BlockHash) {
    fn anchor_block(&self) -> BlockId {
        (*self).into()
    }
}

/// A trait that returns a confirmation height.
///
/// This is typically used to provide an [`Anchor`] implementation the exact confirmation height of
/// the data being anchored.
pub trait ConfirmationHeight {
    /// Returns the confirmation height.
    fn confirmation_height(&self) -> u32;
}

/// Trait that makes an object appendable.
pub trait Append {
    /// Append another object of the same type onto `self`.
    fn append(&mut self, other: Self);
}

impl Append for () {
    fn append(&mut self, _other: Self) {}
}
