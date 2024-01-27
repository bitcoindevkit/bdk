use core::ops::RangeBounds;

use crate::{
    collections::{hash_map::Entry, BTreeMap, BTreeSet, HashMap},
    indexed_tx_graph::Indexer,
};
use bitcoin::{self, OutPoint, Script, ScriptBuf, Transaction, TxOut, Txid};

/// An index storing [`TxOut`]s that have a script pubkey that matches those in a list.
///
/// The basic idea is that you insert script pubkeys you care about into the index with
/// [`insert_spk`] and then when you call [`Indexer::index_tx`] or [`Indexer::index_txout`], the
/// index will look at any txouts you pass in and store and index any txouts matching one of its
/// script pubkeys.
///
/// Each script pubkey is associated with an application-defined index script index `I`, which must be
/// [`Ord`]. Usually, this is used to associate the derivation index of the script pubkey or even a
/// combination of `(keychain, derivation_index)`.
///
/// Note there is no harm in scanning transactions that disappear from the blockchain or were never
/// in there in the first place. `SpkTxOutIndex` is intentionally *monotone* -- you cannot delete or
/// modify txouts that have been indexed. To find out which txouts from the index are actually in the
/// chain or unspent, you must use other sources of information like a [`TxGraph`].
///
/// [`TxOut`]: bitcoin::TxOut
/// [`insert_spk`]: Self::insert_spk
/// [`Ord`]: core::cmp::Ord
/// [`TxGraph`]: crate::tx_graph::TxGraph
#[derive(Clone, Debug)]
pub struct SpkTxOutIndex<I> {
    /// script pubkeys ordered by index
    spks: BTreeMap<I, ScriptBuf>,
    /// A reverse lookup from spk to spk index
    spk_indices: HashMap<ScriptBuf, I>,
    /// The set of unused indexes.
    unused: BTreeSet<I>,
    /// Lookup index and txout by outpoint.
    txouts: BTreeMap<OutPoint, (I, TxOut)>,
    /// Lookup from spk index to outpoints that had that spk
    spk_txouts: BTreeSet<(I, OutPoint)>,
}

impl<I> Default for SpkTxOutIndex<I> {
    fn default() -> Self {
        Self {
            txouts: Default::default(),
            spks: Default::default(),
            spk_indices: Default::default(),
            spk_txouts: Default::default(),
            unused: Default::default(),
        }
    }
}

impl<I: Clone + Ord> Indexer for SpkTxOutIndex<I> {
    type ChangeSet = ();

    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::ChangeSet {
        self.scan_txout(outpoint, txout);
        Default::default()
    }

    fn index_tx(&mut self, tx: &Transaction) -> Self::ChangeSet {
        self.scan(tx);
        Default::default()
    }

    fn initial_changeset(&self) -> Self::ChangeSet {}

    fn apply_changeset(&mut self, _changeset: Self::ChangeSet) {
        // This applies nothing.
    }

    fn is_tx_relevant(&self, tx: &Transaction) -> bool {
        self.is_relevant(tx)
    }
}

impl<I: Clone + Ord> SpkTxOutIndex<I> {
    /// Scans a transaction's outputs for matching script pubkeys.
    ///
    /// Typically, this is used in two situations:
    ///
    /// 1. After loading transaction data from the disk, you may scan over all the txouts to restore all
    /// your txouts.
    /// 2. When getting new data from the chain, you usually scan it before incorporating it into your chain state.
    pub fn scan(&mut self, tx: &Transaction) -> BTreeSet<I> {
        let mut scanned_indices = BTreeSet::new();
        let txid = tx.txid();
        for (i, txout) in tx.output.iter().enumerate() {
            let op = OutPoint::new(txid, i as u32);
            if let Some(spk_i) = self.scan_txout(op, txout) {
                scanned_indices.insert(spk_i.clone());
            }
        }

        scanned_indices
    }

    /// Scan a single `TxOut` for a matching script pubkey and returns the index that matches the
    /// script pubkey (if any).
    pub fn scan_txout(&mut self, op: OutPoint, txout: &TxOut) -> Option<&I> {
        let spk_i = self.spk_indices.get(&txout.script_pubkey);
        if let Some(spk_i) = spk_i {
            self.txouts.insert(op, (spk_i.clone(), txout.clone()));
            self.spk_txouts.insert((spk_i.clone(), op));
            self.unused.remove(spk_i);
        }
        spk_i
    }

    /// Get a reference to the set of indexed outpoints.
    pub fn outpoints(&self) -> &BTreeSet<(I, OutPoint)> {
        &self.spk_txouts
    }

    /// Iterate over all known txouts that spend to tracked script pubkeys.
    pub fn txouts(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&I, OutPoint, &TxOut)> + ExactSizeIterator {
        self.txouts
            .iter()
            .map(|(op, (index, txout))| (index, *op, txout))
    }

    /// Finds all txouts on a transaction that has previously been scanned and indexed.
    pub fn txouts_in_tx(
        &self,
        txid: Txid,
    ) -> impl DoubleEndedIterator<Item = (&I, OutPoint, &TxOut)> {
        self.txouts
            .range(OutPoint::new(txid, u32::MIN)..=OutPoint::new(txid, u32::MAX))
            .map(|(op, (index, txout))| (index, *op, txout))
    }

    /// Iterates over all the outputs with script pubkeys in an index range.
    pub fn outputs_in_range(
        &self,
        range: impl RangeBounds<I>,
    ) -> impl DoubleEndedIterator<Item = (&I, OutPoint)> {
        use bitcoin::hashes::Hash;
        use core::ops::Bound::*;
        let min_op = OutPoint {
            txid: Txid::all_zeros(),
            vout: u32::MIN,
        };
        let max_op = OutPoint {
            txid: Txid::from_byte_array([0xff; Txid::LEN]),
            vout: u32::MAX,
        };

        let start = match range.start_bound() {
            Included(index) => Included((index.clone(), min_op)),
            Excluded(index) => Excluded((index.clone(), max_op)),
            Unbounded => Unbounded,
        };

        let end = match range.end_bound() {
            Included(index) => Included((index.clone(), max_op)),
            Excluded(index) => Excluded((index.clone(), min_op)),
            Unbounded => Unbounded,
        };

        self.spk_txouts.range((start, end)).map(|(i, op)| (i, *op))
    }

    /// Returns the txout and script pubkey index of the `TxOut` at `OutPoint`.
    ///
    /// Returns `None` if the `TxOut` hasn't been scanned or if nothing matching was found there.
    pub fn txout(&self, outpoint: OutPoint) -> Option<(&I, &TxOut)> {
        self.txouts.get(&outpoint).map(|v| (&v.0, &v.1))
    }

    /// Returns the script that has been inserted at the `index`.
    ///
    /// If that index hasn't been inserted yet, it will return `None`.
    pub fn spk_at_index(&self, index: &I) -> Option<&Script> {
        self.spks.get(index).map(|s| s.as_script())
    }

    /// The script pubkeys that are being tracked by the index.
    pub fn all_spks(&self) -> &BTreeMap<I, ScriptBuf> {
        &self.spks
    }

    /// Adds a script pubkey to scan for. Returns `false` and does nothing if spk already exists in the map
    ///
    /// the index will look for outputs spending to this spk whenever it scans new data.
    pub fn insert_spk(&mut self, index: I, spk: ScriptBuf) -> bool {
        match self.spk_indices.entry(spk.clone()) {
            Entry::Vacant(value) => {
                value.insert(index.clone());
                self.spks.insert(index.clone(), spk);
                self.unused.insert(index);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    /// Iterates over all unused script pubkeys in an index range.
    ///
    /// Here, "unused" means that after the script pubkey was stored in the index, the index has
    /// never scanned a transaction output with it.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use bdk_chain::SpkTxOutIndex;
    ///
    /// // imagine our spks are indexed like (keychain, derivation_index).
    /// let txout_index = SpkTxOutIndex::<(u32, u32)>::default();
    /// let all_unused_spks = txout_index.unused_spks(..);
    /// let change_index = 1;
    /// let unused_change_spks =
    ///     txout_index.unused_spks((change_index, u32::MIN)..(change_index, u32::MAX));
    /// ```
    pub fn unused_spks<R>(&self, range: R) -> impl DoubleEndedIterator<Item = (&I, &Script)> + Clone
    where
        R: RangeBounds<I>,
    {
        self.unused
            .range(range)
            .map(move |index| (index, self.spk_at_index(index).expect("must exist")))
    }

    /// Returns whether the script pubkey at `index` has been used or not.
    ///
    /// Here, "unused" means that after the script pubkey was stored in the index, the index has
    /// never scanned a transaction output with it.
    pub fn is_used(&self, index: &I) -> bool {
        self.unused.get(index).is_none()
    }

    /// Marks the script pubkey at `index` as used even though it hasn't seen an output spending to it.
    /// This only affects when the `index` had already been added to `self` and was unused.
    ///
    /// Returns whether the `index` was initially present as `unused`.
    ///
    /// This is useful when you want to reserve a script pubkey for something but don't want to add
    /// the transaction output using it to the index yet. Other callers will consider the `index` used
    /// until you call [`unmark_used`].
    ///
    /// [`unmark_used`]: Self::unmark_used
    pub fn mark_used(&mut self, index: &I) -> bool {
        self.unused.remove(index)
    }

    /// Undoes the effect of [`mark_used`]. Returns whether the `index` is inserted back into
    /// `unused`.
    ///
    /// Note that if `self` has scanned an output with this script pubkey then this will have no
    /// effect.
    ///
    /// [`mark_used`]: Self::mark_used
    pub fn unmark_used(&mut self, index: &I) -> bool {
        // we cannot set the index as unused when it does not exist
        if !self.spks.contains_key(index) {
            return false;
        }
        // we cannot set the index as unused when txouts are indexed under it
        if self.outputs_in_range(index..=index).next().is_some() {
            return false;
        }
        self.unused.insert(index.clone())
    }

    /// Returns the index associated with the script pubkey.
    pub fn index_of_spk(&self, script: &Script) -> Option<&I> {
        self.spk_indices.get(script)
    }

    /// Computes total input value going from script pubkeys in the index (sent) and the total output
    /// value going to script pubkeys in the index (received) in `tx`. For the `sent` to be computed
    /// correctly, the output being spent must have already been scanned by the index. Calculating
    /// received just uses the [`Transaction`] outputs directly, so it will be correct even if it has
    /// not been scanned.
    pub fn sent_and_received(&self, tx: &Transaction) -> (u64, u64) {
        let mut sent = 0;
        let mut received = 0;

        for txin in &tx.input {
            if let Some((_, txout)) = self.txout(txin.previous_output) {
                sent += txout.value.to_sat();
            }
        }
        for txout in &tx.output {
            if self.index_of_spk(&txout.script_pubkey).is_some() {
                received += txout.value.to_sat();
            }
        }

        (sent, received)
    }

    /// Computes the net value that this transaction gives to the script pubkeys in the index and
    /// *takes* from the transaction outputs in the index. Shorthand for calling
    /// [`sent_and_received`] and subtracting sent from received.
    ///
    /// [`sent_and_received`]: Self::sent_and_received
    pub fn net_value(&self, tx: &Transaction) -> i64 {
        let (sent, received) = self.sent_and_received(tx);
        received as i64 - sent as i64
    }

    /// Whether any of the inputs of this transaction spend a txout tracked or whether any output
    /// matches one of our script pubkeys.
    ///
    /// It is easily possible to misuse this method and get false negatives by calling it before you
    /// have scanned the `TxOut`s the transaction is spending. For example, if you want to filter out
    /// all the transactions in a block that are irrelevant, you **must first scan all the
    /// transactions in the block** and only then use this method.
    pub fn is_relevant(&self, tx: &Transaction) -> bool {
        let input_matches = tx
            .input
            .iter()
            .any(|input| self.txouts.contains_key(&input.previous_output));
        let output_matches = tx
            .output
            .iter()
            .any(|output| self.spk_indices.contains_key(&output.script_pubkey));
        input_matches || output_matches
    }
}
