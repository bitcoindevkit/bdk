use core::borrow::Borrow;

use alloc::{borrow::Cow, boxed::Box, rc::Rc, sync::Arc};
use bitcoin::{Block, OutPoint, Transaction, TxOut};

/// Trait to do something with every txout contained in a structure.
///
/// We would prefer just work with things that can give us a `Iterator<Item=(OutPoint, &TxOut)>`
/// here but rust's type system makes it extremely hard to do this (without trait objects).
pub trait ForEachTxOut {
    /// The provided closure `f` will called with each `outpoint/txout` pair.
    fn for_each_txout(&self, f: impl FnMut((OutPoint, &TxOut)));
}

impl ForEachTxOut for Block {
    fn for_each_txout(&self, mut f: impl FnMut((OutPoint, &TxOut))) {
        for tx in self.txdata.iter() {
            tx.for_each_txout(&mut f)
        }
    }
}

/// Trait for things that have a single [`Transaction`] in them.
///
/// This alows polymorphism in structures such as [`TxGraph<T>`] where `T` can be anything that
/// implements `AsTransaction`. You might think that we could just use [`core::convert::AsRef`] for
/// this but the problem is that we need to implement it on `Cow<T>` where `T: AsTransaction` which
/// we can't do with a foreign trait like `AsTransaction`.
///
/// [`Transaction`]: bitcoin::Transaction
/// [`TxGraph<T>`]: crate::tx_graph::TxGraph
pub trait AsTransaction {
    /// Get a reference to the transaction.
    fn as_tx(&self) -> &Transaction;
}

impl AsTransaction for Transaction {
    fn as_tx(&self) -> &Transaction {
        self
    }
}

impl<T: AsTransaction> AsTransaction for Rc<T> {
    fn as_tx(&self) -> &Transaction {
        self.as_ref().as_tx()
    }
}

impl<T: AsTransaction> AsTransaction for Arc<T> {
    fn as_tx(&self) -> &Transaction {
        self.as_ref().as_tx()
    }
}

impl<T: AsTransaction> AsTransaction for Box<T> {
    fn as_tx(&self) -> &Transaction {
        self.as_ref().as_tx()
    }
}

impl<'a, T: AsTransaction + Clone> AsTransaction for Cow<'a, T> {
    fn as_tx(&self) -> &Transaction {
        <Cow<'_, T> as Borrow<T>>::borrow(self).as_tx()
    }
}

impl<T> ForEachTxOut for T
where
    T: AsTransaction,
{
    fn for_each_txout(&self, mut f: impl FnMut((OutPoint, &TxOut))) {
        let tx = self.as_tx();
        let txid = tx.txid();
        for (i, txout) in tx.output.iter().enumerate() {
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

/// A trait like [`core::convert::Into`] for converting one thing into another.
///
/// We use it to convert one transaction type into another so that an update for `T2` can be used on
/// a `TxGraph<T1>` as long as `T2: IntoOwned<T1>`.
///
/// We couldn't use `Into` because we needed to implement it for [`Cow<'a, T>`].
///
/// [`Cow<'a, T>`]: std::borrow::Cow
pub trait IntoOwned<T> {
    /// Converts the provided type into another (owned) type.
    fn into_owned(self) -> T;
}

impl<T> IntoOwned<T> for T {
    fn into_owned(self) -> T {
        self
    }
}

impl<'a, T: Clone> IntoOwned<T> for Cow<'a, T> {
    fn into_owned(self) -> T {
        Cow::into_owned(self)
    }
}

impl<'a, T: Clone> IntoOwned<T> for &'a T {
    fn into_owned(self) -> T {
        self.clone()
    }
}
