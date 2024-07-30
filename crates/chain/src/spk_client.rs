//! Helper types for spk-based blockchain clients.

use crate::{
    collections::BTreeMap, local_chain::CheckPoint, ConfirmationBlockTime, Indexed, TxGraph,
};
use alloc::{borrow::ToOwned, boxed::Box, collections::VecDeque};
use bitcoin::{OutPoint, Script, ScriptBuf, Txid};
use core::fmt::Debug;

/// Data required to perform a spk-based blockchain client sync.
///
/// A client sync fetches relevant chain data for a known list of scripts, transaction ids and
/// outpoints. The sync process also updates the chain from the given [`CheckPoint`].
pub struct SyncRequest<I> {
    /// A checkpoint for the current chain [`LocalChain::tip`].
    /// The sync process will return a new chain update that extends this tip.
    ///
    /// [`LocalChain::tip`]: crate::local_chain::LocalChain::tip
    chain_tip: CheckPoint,
    /// Transactions that spend from or to these indexed script pubkeys.
    spks: VecDeque<(I, ScriptBuf)>,
    /// Transactions with these txids.
    txids: VecDeque<Txid>,
    /// Transactions with these outpoints or spent from these outpoints.
    outpoints: VecDeque<OutPoint>,
    inspect: InspectFn<I>,
    consumed: usize,
}

type InspectFn<I> = Box<dyn FnMut(SyncItem<I>, usize, usize) + Send>;

impl<I> SyncRequest<I> {
    /// Construct a new [`SyncRequest`] from a given `cp` tip.
    pub fn new(cp: CheckPoint) -> Self {
        Self {
            chain_tip: cp,
            spks: Default::default(),
            txids: Default::default(),
            outpoints: Default::default(),
            consumed: Default::default(),
            inspect: Box::new(|_, _, _| {}),
        }
    }

    /// Get the chain tip to update the chain against
    pub fn chain_tip(&self) -> CheckPoint {
        self.chain_tip.clone()
    }

    /// Set the [`Script`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    pub fn add_spks(
        mut self,
        spks: impl IntoIterator<Item = (I, impl ToOwned<Owned = ScriptBuf>)>,
    ) -> Self {
        self.spks
            .extend(spks.into_iter().map(|(i, spk)| (i, spk.to_owned())));
        self
    }

    /// Add transactions to be synced. The sync backend will fetch the current status of the
    /// corresponding transactions.
    pub fn add_txids(mut self, txids: impl IntoIterator<Item = Txid>) -> Self {
        self.txids.extend(txids);
        self
    }

    /// Add the outpoints `OutPoints` to be synced. The sync backend will fetch any transactions
    /// that spend from the outpoint.
    pub fn add_outpoints(mut self, outpoints: impl IntoIterator<Item = OutPoint>) -> Self {
        self.outpoints.extend(outpoints);
        self
    }

    /// Sets the inspect callback for the `SyncRequest`. This will be called for any item pulled
    /// from the sync.
    pub fn inspect<F>(mut self, inspect: F) -> Self
    where
        for<'a> F: FnMut(SyncItem<'a, I>, usize, usize) + 'static + Send,
    {
        self.inspect = Box::new(inspect);
        self
    }

    /// How many items (spks, txids, and outpoints) are there remaining to be processed
    pub fn remaining(&self) -> usize {
        self.spks.len() + self.txids.len() + self.outpoints.len()
    }

    /// Get the next script pubkey to be processed.
    ///
    /// The sync backend should find all transactions spending to or from this script pubkey
    pub fn next_spk(&mut self) -> Option<ScriptBuf> {
        let (index, spk) = self.spks.pop_front()?;
        self._call_inspect(SyncItem::Spk(index, spk.as_script()));
        Some(spk)
    }

    /// Iterate over the script pubkeys to be synced.
    pub fn iter_spks(&mut self) -> impl Iterator<Item = ScriptBuf> + '_ {
        core::iter::from_fn(|| self.next_spk())
    }

    /// Iterate over the transactions to be synced.
    pub fn iter_txids(&mut self) -> impl Iterator<Item = Txid> + '_ {
        core::iter::from_fn(|| self.next_txid())
    }

    /// Iterate over the outpoints to be synced.
    pub fn iter_outpoints(&mut self) -> impl Iterator<Item = OutPoint> + '_ {
        core::iter::from_fn(|| self.next_outpoint())
    }

    /// Get the next txid to be processed.
    ///
    /// The sync backend should get the status of this transaction.
    pub fn next_txid(&mut self) -> Option<Txid> {
        let txid = self.txids.pop_front()?;
        self._call_inspect(SyncItem::Txid(txid));
        Some(txid)
    }

    /// Get the next outpoint to be processed.
    ///
    /// The sync backend should get the status of any transactions spending from this outpoint.
    pub fn next_outpoint(&mut self) -> Option<OutPoint> {
        let op = self.outpoints.pop_front()?;
        self._call_inspect(SyncItem::OutPoint(op));
        Some(op)
    }

    fn _call_inspect(&mut self, item: SyncItem<I>) {
        let remaining_items = self.remaining();
        (self.inspect)(item, self.consumed, remaining_items);
        self.consumed += 1;
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
/// An item reported to [`inspect`]
///
/// [`inspect`]: SyncRequest::inspect
pub enum SyncItem<'a, I> {
    /// Script pubkey sycn item
    Spk(I, &'a Script),
    /// Transaction id sync item
    Txid(Txid),
    /// Transaction outpoint sync item
    OutPoint(OutPoint),
}

/// Data returned from a spk-based blockchain client sync.
///
/// See also [`SyncRequest`].
pub struct SyncResult<A = ConfirmationBlockTime> {
    /// The update to apply to the receiving [`TxGraph`].
    pub graph_update: TxGraph<A>,
    /// The update to apply to the receiving [`LocalChain`](crate::local_chain::LocalChain).
    pub chain_update: CheckPoint,
}

/// Data required to perform a spk-based blockchain client full scan.
///
/// A client full scan iterates through all the scripts for the given keychains, fetching relevant
/// data until some stop gap number of scripts is found that have no data. This operation is
/// generally only used when importing or restoring previously used keychains in which the list of
/// used scripts is not known. The full scan process also updates the chain from the given [`CheckPoint`].
pub struct FullScanRequest<K> {
    /// A checkpoint for the current [`LocalChain::tip`].
    /// The full scan process will return a new chain update that extends this tip.
    ///
    /// [`LocalChain::tip`]: crate::local_chain::LocalChain::tip
    pub chain_tip: CheckPoint,
    /// Iterators of script pubkeys indexed by the keychain index.
    pub spks_by_keychain: BTreeMap<K, Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
}

impl<K: Ord + Clone> FullScanRequest<K> {
    /// Construct a new [`FullScanRequest`] from a given `chain_tip`.
    #[must_use]
    pub fn from_chain_tip(chain_tip: CheckPoint) -> Self {
        Self {
            chain_tip,
            spks_by_keychain: BTreeMap::new(),
        }
    }

    /// Construct a new [`FullScanRequest`] from a given `chain_tip` and `index`.
    ///
    /// Unbounded script pubkey iterators for each keychain (`K`) are extracted using
    /// [`KeychainTxOutIndex::all_unbounded_spk_iters`] and is used to populate the
    /// [`FullScanRequest`].
    ///
    /// [`KeychainTxOutIndex::all_unbounded_spk_iters`]: crate::indexer::keychain_txout::KeychainTxOutIndex::all_unbounded_spk_iters
    #[cfg(feature = "miniscript")]
    #[must_use]
    pub fn from_keychain_txout_index(
        chain_tip: CheckPoint,
        index: &crate::indexer::keychain_txout::KeychainTxOutIndex<K>,
    ) -> Self
    where
        K: core::fmt::Debug,
    {
        let mut req = Self::from_chain_tip(chain_tip);
        for (keychain, spks) in index.all_unbounded_spk_iters() {
            req = req.add_spks_for_keychain(keychain, spks);
        }
        req
    }

    /// Chain on additional [`Script`]s that will be synced against.
    ///
    /// This consumes the [`FullScanRequest`] and returns the updated one.
    #[must_use]
    pub fn add_spks_for_keychain(
        mut self,
        keychain: K,
        spks: impl IntoIterator<IntoIter = impl Iterator<Item = Indexed<ScriptBuf>> + Send + 'static>,
    ) -> Self {
        match self.spks_by_keychain.remove(&keychain) {
            // clippy here suggests to remove `into_iter` from `spks.into_iter()`, but doing so
            // results in a compilation error
            #[allow(clippy::useless_conversion)]
            Some(keychain_spks) => self
                .spks_by_keychain
                .insert(keychain, Box::new(keychain_spks.chain(spks.into_iter()))),
            None => self
                .spks_by_keychain
                .insert(keychain, Box::new(spks.into_iter())),
        };
        self
    }

    /// Add a closure that will be called for every [`Script`] previously added to any keychain in
    /// this request.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn inspect_spks(
        mut self,
        inspect: impl FnMut(K, u32, &Script) + Send + Sync + Clone + 'static,
    ) -> Self
    where
        K: Send + 'static,
    {
        for (keychain, spks) in core::mem::take(&mut self.spks_by_keychain) {
            let mut inspect = inspect.clone();
            self.spks_by_keychain.insert(
                keychain.clone(),
                Box::new(spks.inspect(move |(i, spk)| inspect(keychain.clone(), *i, spk))),
            );
        }
        self
    }
}

/// Data returned from a spk-based blockchain client full scan.
///
/// See also [`FullScanRequest`].
pub struct FullScanResult<K, A = ConfirmationBlockTime> {
    /// The update to apply to the receiving [`LocalChain`](crate::local_chain::LocalChain).
    pub graph_update: TxGraph<A>,
    /// The update to apply to the receiving [`TxGraph`].
    pub chain_update: CheckPoint,
    /// Last active indices for the corresponding keychains (`K`).
    pub last_active_indices: BTreeMap<K, u32>,
}
