use bitcoin::{Block, OutPoint, Transaction, TxOut};

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
