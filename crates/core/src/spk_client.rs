//! Helper types for spk-based blockchain clients.
use crate::{
    alloc::{boxed::Box, collections::VecDeque, vec::Vec},
    collections::{BTreeMap, HashMap, HashSet},
    CheckPoint, ConfirmationBlockTime, Indexed,
};
use bitcoin::{OutPoint, Script, ScriptBuf, Txid};

type InspectSync<I> = dyn FnMut(SyncItem<I>, SyncProgress) + Send + 'static;

type InspectFullScan<K> = dyn FnMut(K, u32, &Script) + Send + 'static;

/// An item reported to the [`inspect`](SyncRequestBuilder::inspect) closure of [`SyncRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyncItem<'i, I> {
    /// Script pubkey sync item.
    Spk(I, &'i Script),
    /// Txid sync item.
    Txid(Txid),
    /// Outpoint sync item.
    OutPoint(OutPoint),
}

impl<I: core::fmt::Debug + core::any::Any> core::fmt::Display for SyncItem<'_, I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SyncItem::Spk(i, spk) => {
                if (i as &dyn core::any::Any).is::<()>() {
                    write!(f, "script '{}'", spk)
                } else {
                    write!(f, "script {:?} '{}'", i, spk)
                }
            }
            SyncItem::Txid(txid) => write!(f, "txid '{}'", txid),
            SyncItem::OutPoint(op) => write!(f, "outpoint '{}'", op),
        }
    }
}

/// The progress of [`SyncRequest`].
#[derive(Debug, Clone)]
pub struct SyncProgress {
    /// Script pubkeys consumed by the request.
    pub spks_consumed: usize,
    /// Script pubkeys remaining in the request.
    pub spks_remaining: usize,
    /// Txids consumed by the request.
    pub txids_consumed: usize,
    /// Txids remaining in the request.
    pub txids_remaining: usize,
    /// Outpoints consumed by the request.
    pub outpoints_consumed: usize,
    /// Outpoints remaining in the request.
    pub outpoints_remaining: usize,
}

impl SyncProgress {
    /// Total items, consumed and remaining, of the request.
    pub fn total(&self) -> usize {
        self.total_spks() + self.total_txids() + self.total_outpoints()
    }

    /// Total script pubkeys, consumed and remaining, of the request.
    pub fn total_spks(&self) -> usize {
        self.spks_consumed + self.spks_remaining
    }

    /// Total txids, consumed and remaining, of the request.
    pub fn total_txids(&self) -> usize {
        self.txids_consumed + self.txids_remaining
    }

    /// Total outpoints, consumed and remaining, of the request.
    pub fn total_outpoints(&self) -> usize {
        self.outpoints_consumed + self.outpoints_remaining
    }

    /// Total consumed items of the request.
    pub fn consumed(&self) -> usize {
        self.spks_consumed + self.txids_consumed + self.outpoints_consumed
    }

    /// Total remaining items of the request.
    pub fn remaining(&self) -> usize {
        self.spks_remaining + self.txids_remaining + self.outpoints_remaining
    }
}

/// [`Script`] with corresponding [`Txid`] histories.
pub struct SpkWithExpectedTxids {
    /// Script pubkey.
    pub spk: ScriptBuf,
    /// Txid history.
    pub txids: HashSet<Txid>,
}

/// Builds a [`SyncRequest`].
#[must_use]
pub struct SyncRequestBuilder<I = ()> {
    inner: SyncRequest<I>,
}

impl<I> Default for SyncRequestBuilder<I> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

impl SyncRequestBuilder<()> {
    /// Add [`Script`]s that will be synced against.
    pub fn spks(self, spks: impl IntoIterator<Item = ScriptBuf>) -> Self {
        self.spks_with_indexes(spks.into_iter().map(|spk| ((), spk)))
    }
}

impl<I> SyncRequestBuilder<I> {
    /// Set the initial chain tip for the sync request.
    ///
    /// This is used to update [`LocalChain`](../../bdk_chain/local_chain/struct.LocalChain.html).
    pub fn chain_tip(mut self, cp: CheckPoint) -> Self {
        self.inner.chain_tip = Some(cp);
        self
    }

    /// Add [`Script`]s coupled with associated indexes that will be synced against.
    ///
    /// # Example
    ///
    /// Sync revealed script pubkeys obtained from a
    /// [`KeychainTxOutIndex`](../../bdk_chain/indexer/keychain_txout/struct.KeychainTxOutIndex.html).
    ///
    /// ```rust
    /// # use bdk_chain::spk_client::SyncRequest;
    /// # use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
    /// # use bdk_chain::miniscript::{Descriptor, DescriptorPublicKey};
    /// # let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
    /// # let (descriptor_a,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
    /// # let (descriptor_b,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)").unwrap();
    /// let mut indexer = KeychainTxOutIndex::<&'static str>::default();
    /// indexer.insert_descriptor("descriptor_a", descriptor_a)?;
    /// indexer.insert_descriptor("descriptor_b", descriptor_b)?;
    ///
    /// /* Assume that the caller does more mutations to the `indexer` here... */
    ///
    /// // Reveal spks for "descriptor_a", then build a sync request. Each spk will be indexed with
    /// // `u32`, which represents the derivation index of the associated spk from "descriptor_a".
    /// let (newly_revealed_spks, _changeset) = indexer
    ///     .reveal_to_target("descriptor_a", 21)
    ///     .expect("keychain must exist");
    /// let _request = SyncRequest::builder()
    ///     .spks_with_indexes(newly_revealed_spks)
    ///     .build();
    ///
    /// // Sync all revealed spks in the indexer. This time, spks may be derived from different
    /// // keychains. Each spk will be indexed with `(&'static str, u32)` where `&'static str` is
    /// // the keychain identifier and `u32` is the derivation index.
    /// let all_revealed_spks = indexer.revealed_spks(..);
    /// let _request = SyncRequest::builder()
    ///     .spks_with_indexes(all_revealed_spks)
    ///     .build();
    /// # Ok::<_, bdk_chain::keychain_txout::InsertDescriptorError<_>>(())
    /// ```
    pub fn spks_with_indexes(mut self, spks: impl IntoIterator<Item = (I, ScriptBuf)>) -> Self {
        self.inner.spks.extend(spks);
        self
    }

    /// Add [`Txid`]s that will be synced against.
    pub fn txids(mut self, txids: impl IntoIterator<Item = Txid>) -> Self {
        self.inner.txids.extend(txids);
        self
    }

    /// Add [`OutPoint`]s that will be synced against.
    pub fn outpoints(mut self, outpoints: impl IntoIterator<Item = OutPoint>) -> Self {
        self.inner.outpoints.extend(outpoints);
        self
    }

    /// Set the closure that will inspect every sync item visited.
    pub fn inspect<F>(mut self, inspect: F) -> Self
    where
        F: FnMut(SyncItem<I>, SyncProgress) + Send + 'static,
    {
        self.inner.inspect = Box::new(inspect);
        self
    }

    /// Build the [`SyncRequest`].
    pub fn build(self) -> SyncRequest<I> {
        self.inner
    }
}

/// Data required to perform a spk-based blockchain client sync.
///
/// A client sync fetches relevant chain data for a known list of scripts, transaction ids and
/// outpoints. The sync process also updates the chain from the given
/// [`chain_tip`](SyncRequestBuilder::chain_tip) (if provided).
///
/// ```rust
/// # use bdk_chain::{bitcoin::{hashes::Hash, ScriptBuf}, local_chain::LocalChain};
/// # use bdk_chain::spk_client::SyncRequest;
/// # let (local_chain, _) = LocalChain::from_genesis_hash(Hash::all_zeros());
/// # let scripts = [ScriptBuf::default(), ScriptBuf::default()];
/// // Construct a sync request.
/// let sync_request = SyncRequest::builder()
///     // Provide chain tip of the local wallet.
///     .chain_tip(local_chain.tip())
///     // Provide list of scripts to scan for transactions against.
///     .spks(scripts)
///     // This is called for every synced item.
///     .inspect(|item, progress| println!("{} (remaining: {})", item, progress.remaining()))
///     // Finish constructing the sync request.
///     .build();
/// ```
#[must_use]
pub struct SyncRequest<I = ()> {
    chain_tip: Option<CheckPoint>,
    spks: VecDeque<(I, ScriptBuf)>,
    spks_consumed: usize,
    spk_histories: HashMap<ScriptBuf, HashSet<Txid>>,
    txids: VecDeque<Txid>,
    txids_consumed: usize,
    outpoints: VecDeque<OutPoint>,
    outpoints_consumed: usize,
    inspect: Box<InspectSync<I>>,
}

impl<I> Default for SyncRequest<I> {
    fn default() -> Self {
        Self {
            chain_tip: None,
            spks: VecDeque::new(),
            spks_consumed: 0,
            spk_histories: HashMap::new(),
            txids: VecDeque::new(),
            txids_consumed: 0,
            outpoints: VecDeque::new(),
            outpoints_consumed: 0,
            inspect: Box::new(|_, _| {}),
        }
    }
}

impl<I> From<SyncRequestBuilder<I>> for SyncRequest<I> {
    fn from(builder: SyncRequestBuilder<I>) -> Self {
        builder.inner
    }
}

impl<I> SyncRequest<I> {
    /// Start building a [`SyncRequest`].
    pub fn builder() -> SyncRequestBuilder<I> {
        SyncRequestBuilder {
            inner: Default::default(),
        }
    }

    /// Get the [`SyncProgress`] of this request.
    pub fn progress(&self) -> SyncProgress {
        SyncProgress {
            spks_consumed: self.spks_consumed,
            spks_remaining: self.spks.len(),
            txids_consumed: self.txids_consumed,
            txids_remaining: self.txids.len(),
            outpoints_consumed: self.outpoints_consumed,
            outpoints_remaining: self.outpoints.len(),
        }
    }

    /// Get the chain tip [`CheckPoint`] of this request (if any).
    pub fn chain_tip(&self) -> Option<CheckPoint> {
        self.chain_tip.clone()
    }

    /// Advances the sync request and returns the next [`ScriptBuf`].
    ///
    /// Returns [`None`] when there are no more scripts remaining in the request.
    pub fn next_spk(&mut self) -> Option<ScriptBuf> {
        let (i, spk) = self.spks.pop_front()?;
        self.spks_consumed += 1;
        self._call_inspect(SyncItem::Spk(i, spk.as_script()));
        Some(spk)
    }

    /// Advances the sync request and returns the next [`ScriptBuf`] with corresponding [`Txid`]
    /// history.
    ///
    /// Returns [`None`] when there are no more scripts remaining in the request.
    pub fn next_spk_with_history(&mut self) -> Option<SpkWithExpectedTxids> {
        let next_spk = self.next_spk()?;
        let spk_history = self
            .spk_histories
            .get(&next_spk)
            .cloned()
            .unwrap_or_default();
        Some(SpkWithExpectedTxids {
            spk: next_spk,
            txids: spk_history,
        })
    }

    /// Advances the sync request and returns the next [`Txid`].
    ///
    /// Returns [`None`] when there are no more txids remaining in the request.
    pub fn next_txid(&mut self) -> Option<Txid> {
        let txid = self.txids.pop_front()?;
        self.txids_consumed += 1;
        self._call_inspect(SyncItem::Txid(txid));
        Some(txid)
    }

    /// Advances the sync request and returns the next [`OutPoint`].
    ///
    /// Returns [`None`] when there are no more outpoints in the request.
    pub fn next_outpoint(&mut self) -> Option<OutPoint> {
        let outpoint = self.outpoints.pop_front()?;
        self.outpoints_consumed += 1;
        self._call_inspect(SyncItem::OutPoint(outpoint));
        Some(outpoint)
    }

    /// Iterate over [`ScriptBuf`]s contained in this request.
    pub fn iter_spks(&mut self) -> impl ExactSizeIterator<Item = ScriptBuf> + '_ {
        SyncIter::<I, ScriptBuf>::new(self)
    }

    /// Iterate over [`ScriptBuf`]s with corresponding [`Txid`] histories contained in this request.
    pub fn iter_spks_with_expected_txids(
        &mut self,
    ) -> impl ExactSizeIterator<Item = SpkWithExpectedTxids> + '_ {
        SyncIter::<I, SpkWithExpectedTxids>::new(self)
    }

    /// Iterate over [`Txid`]s contained in this request.
    pub fn iter_txids(&mut self) -> impl ExactSizeIterator<Item = Txid> + '_ {
        SyncIter::<I, Txid>::new(self)
    }

    /// Iterate over [`OutPoint`]s contained in this request.
    pub fn iter_outpoints(&mut self) -> impl ExactSizeIterator<Item = OutPoint> + '_ {
        SyncIter::<I, OutPoint>::new(self)
    }

    fn _call_inspect(&mut self, item: SyncItem<I>) {
        let progress = self.progress();
        (*self.inspect)(item, progress);
    }
}

/// Data returned from a spk-based blockchain client sync.
///
/// See also [`SyncRequest`].
#[must_use]
#[derive(Debug)]
pub struct SyncResponse<A = ConfirmationBlockTime> {
    /// Relevant transaction data discovered during the scan.
    pub tx_update: crate::TxUpdate<A>,
    /// Changes to the chain discovered during the scan.
    pub chain_update: Option<CheckPoint>,
}

impl<A> Default for SyncResponse<A> {
    fn default() -> Self {
        Self {
            tx_update: Default::default(),
            chain_update: Default::default(),
        }
    }
}

/// Builds a [`FullScanRequest`].
#[must_use]
pub struct FullScanRequestBuilder<K> {
    inner: FullScanRequest<K>,
}

impl<K> Default for FullScanRequestBuilder<K> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
        }
    }
}

impl<K: Ord> FullScanRequestBuilder<K> {
    /// Set the initial chain tip for the full scan request.
    ///
    /// This is used to update [`LocalChain`](../../bdk_chain/local_chain/struct.LocalChain.html).
    pub fn chain_tip(mut self, tip: CheckPoint) -> Self {
        self.inner.chain_tip = Some(tip);
        self
    }

    /// Set the spk iterator for a given `keychain`.
    pub fn spks_for_keychain(
        mut self,
        keychain: K,
        spks: impl IntoIterator<IntoIter = impl Iterator<Item = Indexed<ScriptBuf>> + Send + 'static>,
    ) -> Self {
        self.inner
            .spks_by_keychain
            .insert(keychain, Box::new(spks.into_iter()));
        self
    }

    /// Set the closure that will inspect every sync item visited.
    pub fn inspect<F>(mut self, inspect: F) -> Self
    where
        F: FnMut(K, u32, &Script) + Send + 'static,
    {
        self.inner.inspect = Box::new(inspect);
        self
    }

    /// Build the [`FullScanRequest`].
    pub fn build(self) -> FullScanRequest<K> {
        self.inner
    }
}

/// Data required to perform a spk-based blockchain client full scan.
///
/// A client full scan iterates through all the scripts for the given keychains, fetching relevant
/// data until some stop gap number of scripts is found that have no data. This operation is
/// generally only used when importing or restoring previously used keychains in which the list of
/// used scripts is not known. The full scan process also updates the chain from the given
/// [`chain_tip`](FullScanRequestBuilder::chain_tip) (if provided).
#[must_use]
pub struct FullScanRequest<K> {
    chain_tip: Option<CheckPoint>,
    spks_by_keychain: BTreeMap<K, Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
    inspect: Box<InspectFullScan<K>>,
}

impl<K> From<FullScanRequestBuilder<K>> for FullScanRequest<K> {
    fn from(builder: FullScanRequestBuilder<K>) -> Self {
        builder.inner
    }
}

impl<K> Default for FullScanRequest<K> {
    fn default() -> Self {
        Self {
            chain_tip: None,
            spks_by_keychain: Default::default(),
            inspect: Box::new(|_, _, _| {}),
        }
    }
}

impl<K: Ord + Clone> FullScanRequest<K> {
    /// Start building a [`FullScanRequest`].
    pub fn builder() -> FullScanRequestBuilder<K> {
        FullScanRequestBuilder {
            inner: Self::default(),
        }
    }

    /// Get the chain tip [`CheckPoint`] of this request (if any).
    pub fn chain_tip(&self) -> Option<CheckPoint> {
        self.chain_tip.clone()
    }

    /// List all keychains contained in this request.
    pub fn keychains(&self) -> Vec<K> {
        self.spks_by_keychain.keys().cloned().collect()
    }

    /// Advances the full scan request and returns the next indexed [`ScriptBuf`] of the given
    /// `keychain`.
    pub fn next_spk(&mut self, keychain: K) -> Option<Indexed<ScriptBuf>> {
        self.iter_spks(keychain).next()
    }

    /// Iterate over indexed [`ScriptBuf`]s contained in this request of the given `keychain`.
    pub fn iter_spks(&mut self, keychain: K) -> impl Iterator<Item = Indexed<ScriptBuf>> + '_ {
        let spks = self.spks_by_keychain.get_mut(&keychain);
        let inspect = &mut self.inspect;
        KeychainSpkIter {
            keychain,
            spks,
            inspect,
        }
    }
}

/// Data returned from a spk-based blockchain client full scan.
///
/// See also [`FullScanRequest`].
#[must_use]
#[derive(Debug)]
pub struct FullScanResponse<K, A = ConfirmationBlockTime> {
    /// Relevant transaction data discovered during the scan.
    pub tx_update: crate::TxUpdate<A>,
    /// Last active indices for the corresponding keychains (`K`). An index is active if it had a
    /// transaction associated with the script pubkey at that index.
    pub last_active_indices: BTreeMap<K, u32>,
    /// Changes to the chain discovered during the scan.
    pub chain_update: Option<CheckPoint>,
}

impl<K, A> Default for FullScanResponse<K, A> {
    fn default() -> Self {
        Self {
            tx_update: Default::default(),
            chain_update: Default::default(),
            last_active_indices: Default::default(),
        }
    }
}

struct KeychainSpkIter<'r, K> {
    keychain: K,
    spks: Option<&'r mut Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
    inspect: &'r mut Box<InspectFullScan<K>>,
}

impl<K: Ord + Clone> Iterator for KeychainSpkIter<'_, K> {
    type Item = Indexed<ScriptBuf>;

    fn next(&mut self) -> Option<Self::Item> {
        let (i, spk) = self.spks.as_mut()?.next()?;
        (*self.inspect)(self.keychain.clone(), i, &spk);
        Some((i, spk))
    }
}

struct SyncIter<'r, I, Item> {
    request: &'r mut SyncRequest<I>,
    marker: core::marker::PhantomData<Item>,
}

impl<'r, I, Item> SyncIter<'r, I, Item> {
    fn new(request: &'r mut SyncRequest<I>) -> Self {
        Self {
            request,
            marker: core::marker::PhantomData,
        }
    }
}

impl<'r, I, Item> ExactSizeIterator for SyncIter<'r, I, Item> where SyncIter<'r, I, Item>: Iterator {}

impl<I> Iterator for SyncIter<'_, I, ScriptBuf> {
    type Item = ScriptBuf;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_spk()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.spks.len();
        (remaining, Some(remaining))
    }
}

impl<I> Iterator for SyncIter<'_, I, SpkWithExpectedTxids> {
    type Item = SpkWithExpectedTxids;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_spk_with_history()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.spks.len();
        (remaining, Some(remaining))
    }
}

impl<I> Iterator for SyncIter<'_, I, Txid> {
    type Item = Txid;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_txid()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.txids.len();
        (remaining, Some(remaining))
    }
}

impl<I> Iterator for SyncIter<'_, I, OutPoint> {
    type Item = OutPoint;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_outpoint()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.outpoints.len();
        (remaining, Some(remaining))
    }
}
