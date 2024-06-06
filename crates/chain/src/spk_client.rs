//! Helper types for spk-based blockchain clients.

use crate::{
    collections::BTreeMap, local_chain::CheckPoint, ConfirmationTimeHeightAnchor, TxGraph,
};
use alloc::{boxed::Box, vec::Vec};
use bitcoin::{OutPoint, Script, ScriptBuf, Txid};
use core::{fmt::Debug, marker::PhantomData, ops::RangeBounds};

/// Data required to perform a spk-based blockchain client sync.
///
/// A client sync fetches relevant chain data for a known list of scripts, transaction ids and
/// outpoints. The sync process also updates the chain from the given [`CheckPoint`].
pub struct SyncRequest {
    /// A checkpoint for the current chain [`LocalChain::tip`].
    /// The sync process will return a new chain update that extends this tip.
    ///
    /// [`LocalChain::tip`]: crate::local_chain::LocalChain::tip
    pub chain_tip: CheckPoint,
    /// Transactions that spend from or to these indexed script pubkeys.
    pub spks: Box<dyn ExactSizeIterator<Item = ScriptBuf> + Send>,
    /// Transactions with these txids.
    pub txids: Box<dyn ExactSizeIterator<Item = Txid> + Send>,
    /// Transactions with these outpoints or spent from these outpoints.
    pub outpoints: Box<dyn ExactSizeIterator<Item = OutPoint> + Send>,
}

impl SyncRequest {
    /// Construct a new [`SyncRequest`] from a given `cp` tip.
    pub fn from_chain_tip(cp: CheckPoint) -> Self {
        Self {
            chain_tip: cp,
            spks: Box::new(core::iter::empty()),
            txids: Box::new(core::iter::empty()),
            outpoints: Box::new(core::iter::empty()),
        }
    }

    /// Set the [`Script`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn set_spks(
        mut self,
        spks: impl IntoIterator<IntoIter = impl ExactSizeIterator<Item = ScriptBuf> + Send + 'static>,
    ) -> Self {
        self.spks = Box::new(spks.into_iter());
        self
    }

    /// Set the [`Txid`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn set_txids(
        mut self,
        txids: impl IntoIterator<IntoIter = impl ExactSizeIterator<Item = Txid> + Send + 'static>,
    ) -> Self {
        self.txids = Box::new(txids.into_iter());
        self
    }

    /// Set the [`OutPoint`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn set_outpoints(
        mut self,
        outpoints: impl IntoIterator<
            IntoIter = impl ExactSizeIterator<Item = OutPoint> + Send + 'static,
        >,
    ) -> Self {
        self.outpoints = Box::new(outpoints.into_iter());
        self
    }

    /// Chain on additional [`Script`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn chain_spks(
        mut self,
        spks: impl IntoIterator<
            IntoIter = impl ExactSizeIterator<Item = ScriptBuf> + Send + 'static,
            Item = ScriptBuf,
        >,
    ) -> Self {
        self.spks = Box::new(ExactSizeChain::new(self.spks, spks.into_iter()));
        self
    }

    /// Chain on additional [`Txid`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn chain_txids(
        mut self,
        txids: impl IntoIterator<
            IntoIter = impl ExactSizeIterator<Item = Txid> + Send + 'static,
            Item = Txid,
        >,
    ) -> Self {
        self.txids = Box::new(ExactSizeChain::new(self.txids, txids.into_iter()));
        self
    }

    /// Chain on additional [`OutPoint`]s that will be synced against.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn chain_outpoints(
        mut self,
        outpoints: impl IntoIterator<
            IntoIter = impl ExactSizeIterator<Item = OutPoint> + Send + 'static,
            Item = OutPoint,
        >,
    ) -> Self {
        self.outpoints = Box::new(ExactSizeChain::new(self.outpoints, outpoints.into_iter()));
        self
    }

    /// Add a closure that will be called for [`Script`]s previously added to this request.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn inspect_spks(
        mut self,
        mut inspect: impl FnMut(&Script) + Send + Sync + 'static,
    ) -> Self {
        self.spks = Box::new(self.spks.inspect(move |spk| inspect(spk)));
        self
    }

    /// Add a closure that will be called for [`Txid`]s previously added to this request.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn inspect_txids(mut self, mut inspect: impl FnMut(&Txid) + Send + Sync + 'static) -> Self {
        self.txids = Box::new(self.txids.inspect(move |txid| inspect(txid)));
        self
    }

    /// Add a closure that will be called for [`OutPoint`]s previously added to this request.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn inspect_outpoints(
        mut self,
        mut inspect: impl FnMut(&OutPoint) + Send + Sync + 'static,
    ) -> Self {
        self.outpoints = Box::new(self.outpoints.inspect(move |op| inspect(op)));
        self
    }

    /// Populate the request with revealed script pubkeys from `index` with the given `spk_range`.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[cfg(feature = "miniscript")]
    #[must_use]
    pub fn populate_with_revealed_spks<K: Clone + Ord + Debug + Send + Sync>(
        self,
        index: &crate::keychain::KeychainTxOutIndex<K>,
        spk_range: impl RangeBounds<K>,
    ) -> Self {
        use alloc::borrow::ToOwned;
        self.chain_spks(
            index
                .revealed_spks(spk_range)
                .map(|(_, spk)| spk.to_owned())
                .collect::<Vec<_>>(),
        )
    }
}

/// Data returned from a spk-based blockchain client sync.
///
/// See also [`SyncRequest`].
pub struct SyncResult<A = ConfirmationTimeHeightAnchor> {
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
    pub spks_by_keychain: BTreeMap<K, Box<dyn Iterator<Item = (u32, ScriptBuf)> + Send>>,
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
    /// [`KeychainTxOutIndex::all_unbounded_spk_iters`]: crate::keychain::KeychainTxOutIndex::all_unbounded_spk_iters
    #[cfg(feature = "miniscript")]
    #[must_use]
    pub fn from_keychain_txout_index(
        chain_tip: CheckPoint,
        index: &crate::keychain::KeychainTxOutIndex<K>,
    ) -> Self
    where
        K: Debug,
    {
        let mut req = Self::from_chain_tip(chain_tip);
        for (keychain, spks) in index.all_unbounded_spk_iters() {
            req = req.set_spks_for_keychain(keychain, spks);
        }
        req
    }

    /// Set the [`Script`]s for a given `keychain`.
    ///
    /// This consumes the [`FullScanRequest`] and returns the updated one.
    #[must_use]
    pub fn set_spks_for_keychain(
        mut self,
        keychain: K,
        spks: impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send + 'static>,
    ) -> Self {
        self.spks_by_keychain
            .insert(keychain, Box::new(spks.into_iter()));
        self
    }

    /// Chain on additional [`Script`]s that will be synced against.
    ///
    /// This consumes the [`FullScanRequest`] and returns the updated one.
    #[must_use]
    pub fn chain_spks_for_keychain(
        mut self,
        keychain: K,
        spks: impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send + 'static>,
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
    pub fn inspect_spks_for_all_keychains(
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

    /// Add a closure that will be called for every [`Script`] previously added to a given
    /// `keychain` in this request.
    ///
    /// This consumes the [`SyncRequest`] and returns the updated one.
    #[must_use]
    pub fn inspect_spks_for_keychain(
        mut self,
        keychain: K,
        mut inspect: impl FnMut(u32, &Script) + Send + Sync + 'static,
    ) -> Self
    where
        K: Send + 'static,
    {
        if let Some(spks) = self.spks_by_keychain.remove(&keychain) {
            self.spks_by_keychain.insert(
                keychain,
                Box::new(spks.inspect(move |(i, spk)| inspect(*i, spk))),
            );
        }
        self
    }
}

/// Data returned from a spk-based blockchain client full scan.
///
/// See also [`FullScanRequest`].
pub struct FullScanResult<K, A = ConfirmationTimeHeightAnchor> {
    /// The update to apply to the receiving [`LocalChain`](crate::local_chain::LocalChain).
    pub graph_update: TxGraph<A>,
    /// The update to apply to the receiving [`TxGraph`].
    pub chain_update: CheckPoint,
    /// Last active indices for the corresponding keychains (`K`).
    pub last_active_indices: BTreeMap<K, u32>,
}

/// A version of [`core::iter::Chain`] which can combine two [`ExactSizeIterator`]s to form a new
/// [`ExactSizeIterator`].
///
/// The danger of this is explained in [the `ExactSizeIterator` docs]
/// (https://doc.rust-lang.org/core/iter/trait.ExactSizeIterator.html#when-shouldnt-an-adapter-be-exactsizeiterator).
/// This does not apply here since it would be impossible to scan an item count that overflows
/// `usize` anyway.
struct ExactSizeChain<A, B, I> {
    a: Option<A>,
    b: Option<B>,
    i: PhantomData<I>,
}

impl<A, B, I> ExactSizeChain<A, B, I> {
    fn new(a: A, b: B) -> Self {
        ExactSizeChain {
            a: Some(a),
            b: Some(b),
            i: PhantomData,
        }
    }
}

impl<A, B, I> Iterator for ExactSizeChain<A, B, I>
where
    A: Iterator<Item = I>,
    B: Iterator<Item = I>,
{
    type Item = I;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(a) = &mut self.a {
            let item = a.next();
            if item.is_some() {
                return item;
            }
            self.a = None;
        }
        if let Some(b) = &mut self.b {
            let item = b.next();
            if item.is_some() {
                return item;
            }
            self.b = None;
        }
        None
    }
}

impl<A, B, I> ExactSizeIterator for ExactSizeChain<A, B, I>
where
    A: ExactSizeIterator<Item = I>,
    B: ExactSizeIterator<Item = I>,
{
    fn len(&self) -> usize {
        let a_len = self.a.as_ref().map(|a| a.len()).unwrap_or(0);
        let b_len = self.b.as_ref().map(|a| a.len()).unwrap_or(0);
        a_len + b_len
    }
}
