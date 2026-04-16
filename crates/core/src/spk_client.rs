//! Helper types for spk-based blockchain clients.
use crate::{
    alloc::{boxed::Box, collections::VecDeque, vec::Vec},
    collections::{BTreeMap, HashMap, HashSet},
    CheckPoint, ConfirmationBlockTime, Indexed,
};
use bitcoin::{BlockHash, OutPoint, Script, ScriptBuf, Txid};

type InspectSync<I> = dyn FnMut(SyncItem<I>, SyncProgress) + Send + 'static;

type InspectFullScan<K> = dyn FnMut(K, u32, &Script) + Send + 'static;

const DEFAULT_STOP_GAP: usize = 20;

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
                    write!(f, "script '{spk}'")
                } else {
                    write!(f, "script {i:?} '{spk}'")
                }
            }
            SyncItem::Txid(txid) => write!(f, "txid '{txid}'"),
            SyncItem::OutPoint(op) => write!(f, "outpoint '{op}'"),
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

/// [`Script`] with expected [`Txid`] histories.
#[derive(Debug, Clone)]
pub struct SpkWithExpectedTxids {
    /// Script pubkey.
    pub spk: ScriptBuf,

    /// [`Txid`]s that we expect to appear in the chain source's spk history response.
    ///
    /// Any transaction listed here that is missing from the spk history response should be
    /// considered evicted from the mempool.
    pub expected_txids: HashSet<Txid>,
}

impl From<ScriptBuf> for SpkWithExpectedTxids {
    fn from(spk: ScriptBuf) -> Self {
        Self {
            spk,
            expected_txids: HashSet::new(),
        }
    }
}

/// Builds a [`SyncRequest`].
///
/// Construct with [`SyncRequest::builder`].
#[must_use]
pub struct SyncRequestBuilder<I = (), D = BlockHash> {
    inner: SyncRequest<I, D>,
}

impl SyncRequestBuilder<()> {
    /// Add [`Script`]s that will be synced against.
    pub fn spks(self, spks: impl IntoIterator<Item = ScriptBuf>) -> Self {
        self.spks_with_indexes(spks.into_iter().map(|spk| ((), spk)))
    }
}

impl<I, D> SyncRequestBuilder<I, D> {
    /// Set the initial chain tip for the sync request.
    ///
    /// This is used to update [`LocalChain`](../../bdk_chain/local_chain/struct.LocalChain.html).
    pub fn chain_tip(mut self, cp: CheckPoint<D>) -> Self {
        self.inner.chain_tip = Some(cp);
        self
    }

    /// Add [`Script`]s coupled with associated indexes that will be synced against.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use bdk_chain::bitcoin::BlockHash;
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
    /// let _request: SyncRequest<u32, BlockHash> = SyncRequest::builder()
    ///     .spks_with_indexes(newly_revealed_spks)
    ///     .build();
    ///
    /// // Sync all revealed spks in the indexer. This time, spks may be derived from different
    /// // keychains. Each spk will be indexed with `(&str, u32)` where `&str` is the keychain
    /// // identifier and `u32` is the derivation index.
    /// let all_revealed_spks = indexer.revealed_spks(..);
    /// let _request: SyncRequest<(&str, u32), BlockHash> = SyncRequest::builder()
    ///     .spks_with_indexes(all_revealed_spks)
    ///     .build();
    /// # Ok::<_, bdk_chain::keychain_txout::InsertDescriptorError<_>>(())
    /// ```
    pub fn spks_with_indexes(mut self, spks: impl IntoIterator<Item = (I, ScriptBuf)>) -> Self {
        self.inner.spks.extend(spks);
        self
    }

    /// Add transactions that are expected to exist under the given spks.
    ///
    /// This is useful for detecting a malicious replacement of an incoming transaction.
    pub fn expected_spk_txids(mut self, txs: impl IntoIterator<Item = (ScriptBuf, Txid)>) -> Self {
        for (spk, txid) in txs {
            self.inner
                .spk_expected_txids
                .entry(spk)
                .or_default()
                .insert(txid);
        }
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
    pub fn build(self) -> SyncRequest<I, D> {
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
/// # use std::io::{self, Write};
/// # use bdk_chain::{bitcoin::{hashes::Hash, ScriptBuf}, local_chain::LocalChain};
/// # use bdk_chain::spk_client::SyncRequest;
/// # let (local_chain, _) = LocalChain::from_genesis(Hash::all_zeros());
/// # let scripts = [ScriptBuf::default(), ScriptBuf::default()];
/// // Construct a sync request.
/// let sync_request = SyncRequest::builder()
///     // Provide chain tip of the local wallet.
///     .chain_tip(local_chain.tip())
///     // Provide list of scripts to scan for transactions against.
///     .spks(scripts)
///     // This is called for every synced item.
///     .inspect(|item, progress| {
///         let pc = (100.0 * progress.consumed() as f32) / progress.total() as f32;
///         match item {
///             // In this example I = (), so the first field of Spk is unit.
///             bdk_chain::spk_client::SyncItem::Spk((), spk) => {
///                 eprintln!("[ SCANNING {pc:03.0}% ] script {}", spk);
///             }
///             bdk_chain::spk_client::SyncItem::Txid(txid) => {
///                 eprintln!("[ SCANNING {pc:03.0}% ] txid {}", txid);
///             }
///             bdk_chain::spk_client::SyncItem::OutPoint(op) => {
///                 eprintln!("[ SCANNING {pc:03.0}% ] outpoint {}", op);
///             }
///         }
///         let _ = io::stderr().flush();
///     })
///     // Finish constructing the sync request.
///     .build();
/// ```
#[must_use]
pub struct SyncRequest<I = (), D = BlockHash> {
    start_time: u64,
    chain_tip: Option<CheckPoint<D>>,
    spks: VecDeque<(I, ScriptBuf)>,
    spks_consumed: usize,
    spk_expected_txids: HashMap<ScriptBuf, HashSet<Txid>>,
    txids: VecDeque<Txid>,
    txids_consumed: usize,
    outpoints: VecDeque<OutPoint>,
    outpoints_consumed: usize,
    inspect: Box<InspectSync<I>>,
}

impl<I, D> From<SyncRequestBuilder<I, D>> for SyncRequest<I, D> {
    fn from(builder: SyncRequestBuilder<I, D>) -> Self {
        builder.inner
    }
}

impl<I, D> SyncRequest<I, D> {
    /// Start building [`SyncRequest`] with a given `start_time`.
    ///
    /// `start_time` specifies the start time of sync. Chain sources can use this value to set
    /// [`TxUpdate::seen_ats`](crate::TxUpdate::seen_ats) for mempool transactions. A transaction
    /// without any `seen_ats` is assumed to be unseen in the mempool.
    ///
    /// Use [`SyncRequest::builder`] to use the current timestamp as `start_time` (this requires
    /// `feature = "std"`).
    pub fn builder_at(start_time: u64) -> SyncRequestBuilder<I, D> {
        SyncRequestBuilder {
            inner: Self {
                start_time,
                chain_tip: None,
                spks: VecDeque::new(),
                spks_consumed: 0,
                spk_expected_txids: HashMap::new(),
                txids: VecDeque::new(),
                txids_consumed: 0,
                outpoints: VecDeque::new(),
                outpoints_consumed: 0,
                inspect: Box::new(|_, _| ()),
            },
        }
    }

    /// Start building [`SyncRequest`] with the current timestamp as the `start_time`.
    ///
    /// Use [`SyncRequest::builder_at`] to manually set the `start_time`, or if `feature = "std"`
    /// is not available.
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    pub fn builder() -> SyncRequestBuilder<I, D> {
        let start_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("failed to get current timestamp")
            .as_secs();
        Self::builder_at(start_time)
    }

    /// When the sync-request was initiated.
    pub fn start_time(&self) -> u64 {
        self.start_time
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
    pub fn chain_tip(&self) -> Option<CheckPoint<D>> {
        self.chain_tip.clone()
    }

    /// Advances the sync request and returns the next [`ScriptBuf`] with corresponding [`Txid`]
    /// history.
    ///
    /// Returns [`None`] when there are no more scripts remaining in the request.
    pub fn next_spk_with_expected_txids(&mut self) -> Option<SpkWithExpectedTxids> {
        let (i, next_spk) = self.spks.pop_front()?;
        self.spks_consumed += 1;
        self._call_inspect(SyncItem::Spk(i, next_spk.as_script()));
        let spk_history = self
            .spk_expected_txids
            .get(&next_spk)
            .cloned()
            .unwrap_or_default();
        Some(SpkWithExpectedTxids {
            spk: next_spk,
            expected_txids: spk_history,
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

    /// Iterate over [`ScriptBuf`]s with corresponding [`Txid`] histories contained in this request.
    pub fn iter_spks_with_expected_txids(
        &mut self,
    ) -> impl ExactSizeIterator<Item = SpkWithExpectedTxids> + '_ {
        SyncIter::<I, D, SpkWithExpectedTxids>::new(self)
    }

    /// Iterate over [`Txid`]s contained in this request.
    pub fn iter_txids(&mut self) -> impl ExactSizeIterator<Item = Txid> + '_ {
        SyncIter::<I, D, Txid>::new(self)
    }

    /// Iterate over [`OutPoint`]s contained in this request.
    pub fn iter_outpoints(&mut self) -> impl ExactSizeIterator<Item = OutPoint> + '_ {
        SyncIter::<I, D, OutPoint>::new(self)
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
pub struct SyncResponse<A = ConfirmationBlockTime, D = BlockHash> {
    /// Relevant transaction data discovered during the scan.
    pub tx_update: crate::TxUpdate<A>,
    /// Changes to the chain discovered during the scan.
    pub chain_update: Option<CheckPoint<D>>,
}

impl<A, D> Default for SyncResponse<A, D> {
    fn default() -> Self {
        Self {
            tx_update: Default::default(),
            chain_update: Default::default(),
        }
    }
}

impl<A> SyncResponse<A> {
    /// Returns true if the `SyncResponse` is empty.
    pub fn is_empty(&self) -> bool {
        self.tx_update.is_empty() && self.chain_update.is_none()
    }
}

/// Builds a [`FullScanRequest`].
///
/// Construct with [`FullScanRequest::builder`].
#[must_use]
pub struct FullScanRequestBuilder<K, D = BlockHash> {
    inner: FullScanRequest<K, D>,
}

impl<K: Ord, D> FullScanRequestBuilder<K, D> {
    /// Set the initial chain tip for the full scan request.
    ///
    /// This is used to update [`LocalChain`](../../bdk_chain/local_chain/struct.LocalChain.html).
    pub fn chain_tip(mut self, tip: CheckPoint<D>) -> Self {
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
    pub fn build(self) -> FullScanRequest<K, D> {
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
pub struct FullScanRequest<K, D = BlockHash> {
    start_time: u64,
    chain_tip: Option<CheckPoint<D>>,
    spks_by_keychain: BTreeMap<K, Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
    inspect: Box<InspectFullScan<K>>,
}

impl<K, D> From<FullScanRequestBuilder<K, D>> for FullScanRequest<K, D> {
    fn from(builder: FullScanRequestBuilder<K, D>) -> Self {
        builder.inner
    }
}

impl<K: Ord + Clone, D> FullScanRequest<K, D> {
    /// Start building a [`FullScanRequest`] with a given `start_time`.
    ///
    /// `start_time` specifies the start time of sync. Chain sources can use this value to set
    /// [`TxUpdate::seen_ats`](crate::TxUpdate::seen_ats) for mempool transactions. A transaction
    /// without any `seen_ats` is assumed to be unseen in the mempool.
    ///
    /// Use [`FullScanRequest::builder`] to use the current timestamp as `start_time` (this
    /// requires `feature = "std`).
    pub fn builder_at(start_time: u64) -> FullScanRequestBuilder<K, D> {
        FullScanRequestBuilder {
            inner: Self {
                start_time,
                chain_tip: None,
                spks_by_keychain: BTreeMap::new(),
                inspect: Box::new(|_, _, _| ()),
            },
        }
    }

    /// Start building a [`FullScanRequest`] with the current timestamp as the `start_time`.
    ///
    /// Use [`FullScanRequest::builder_at`] to manually set the `start_time`, or if `feature =
    /// "std"` is not available.
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    pub fn builder() -> FullScanRequestBuilder<K, D> {
        let start_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("failed to get current timestamp")
            .as_secs();
        Self::builder_at(start_time)
    }

    /// When the full-scan-request was initiated.
    pub fn start_time(&self) -> u64 {
        self.start_time
    }

    /// Get the chain tip [`CheckPoint`] of this request (if any).
    pub fn chain_tip(&self) -> Option<CheckPoint<D>> {
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
pub struct FullScanResponse<K, A = ConfirmationBlockTime, D = BlockHash> {
    /// Relevant transaction data discovered during the scan.
    pub tx_update: crate::TxUpdate<A>,
    /// Last active indices for the corresponding keychains (`K`). An index is active if it had a
    /// transaction associated with the script pubkey at that index.
    pub last_active_indices: BTreeMap<K, u32>,
    /// Changes to the chain discovered during the scan.
    pub chain_update: Option<CheckPoint<D>>,
}

impl<K, A, D> Default for FullScanResponse<K, A, D> {
    fn default() -> Self {
        Self {
            tx_update: Default::default(),
            chain_update: Default::default(),
            last_active_indices: Default::default(),
        }
    }
}

impl<K, A> FullScanResponse<K, A> {
    /// Returns true if the `FullScanResponse` is empty.
    pub fn is_empty(&self) -> bool {
        self.tx_update.is_empty()
            && self.last_active_indices.is_empty()
            && self.chain_update.is_none()
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

struct SyncIter<'r, I, D, Item> {
    request: &'r mut SyncRequest<I, D>,
    marker: core::marker::PhantomData<Item>,
}

impl<'r, I, D, Item> SyncIter<'r, I, D, Item> {
    fn new(request: &'r mut SyncRequest<I, D>) -> Self {
        Self {
            request,
            marker: core::marker::PhantomData,
        }
    }
}

impl<'r, I, D, Item> ExactSizeIterator for SyncIter<'r, I, D, Item> where
    SyncIter<'r, I, D, Item>: Iterator
{
}

impl<I, D> Iterator for SyncIter<'_, I, D, SpkWithExpectedTxids> {
    type Item = SpkWithExpectedTxids;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_spk_with_expected_txids()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.spks.len();
        (remaining, Some(remaining))
    }
}

impl<I, D> Iterator for SyncIter<'_, I, D, Txid> {
    type Item = Txid;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_txid()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.txids.len();
        (remaining, Some(remaining))
    }
}

impl<I, D> Iterator for SyncIter<'_, I, D, OutPoint> {
    type Item = OutPoint;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_outpoint()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.outpoints.len();
        (remaining, Some(remaining))
    }
}

/// An item reported to the [`inspect`](ScanRequestBuilder::inspect) closure of [`ScanRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScanItem<'i, K, I> {
    /// A keychain discovery script being scanned.
    Discovery(K, u32, &'i Script),
    /// A script being synced.
    Spk(I, &'i Script),
    /// A txid being synced.
    Txid(Txid),
    /// An outpoint being synced.
    OutPoint(OutPoint),
}

impl<K: core::fmt::Debug + core::any::Any, I: core::fmt::Debug + core::any::Any> core::fmt::Display
    for ScanItem<'_, K, I>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ScanItem::Discovery(k, i, spk) => {
                if (k as &dyn core::any::Any).is::<()>() {
                    write!(f, "discovery script #{i} '{spk}'")
                } else {
                    write!(f, "discovery script {k:?}#{i} '{spk}'")
                }
            }
            ScanItem::Spk(i, spk) => {
                if (i as &dyn core::any::Any).is::<()>() {
                    write!(f, "script '{spk}'")
                } else {
                    write!(f, "script {i:?} '{spk}'")
                }
            }
            ScanItem::Txid(txid) => write!(f, "txid '{txid}'"),
            ScanItem::OutPoint(op) => write!(f, "outpoint '{op}'"),
        }
    }
}

/// Progress of a [`ScanRequest`].
///
/// Discovery progress only tracks consumed count (remaining is unknown because they are unbounded).
/// Explicit sync progress tracks both consumed and remaining.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanProgress {
    /// Scripts consumed by keychain discovery.
    pub discovery_consumed: usize,
    /// Script pubkeys consumed by explicit sync.
    pub spks_consumed: usize,
    /// Script pubkeys remaining in explicit sync.
    pub spks_remaining: usize,
    /// Txids consumed by explicit sync.
    pub txids_consumed: usize,
    /// Txids remaining in explicit sync.
    pub txids_remaining: usize,
    /// Outpoints consumed by explicit sync.
    pub outpoints_consumed: usize,
    /// Outpoints remaining in explicit sync.
    pub outpoints_remaining: usize,
}

impl ScanProgress {
    /// Total explicit sync items (consumed + remaining).
    pub fn explicit_total(&self) -> usize {
        self.explicit_consumed() + self.explicit_remaining()
    }

    /// Total explicit sync items consumed.
    pub fn explicit_consumed(&self) -> usize {
        self.spks_consumed + self.txids_consumed + self.outpoints_consumed
    }

    /// Total explicit sync items remaining.
    pub fn explicit_remaining(&self) -> usize {
        self.spks_remaining + self.txids_remaining + self.outpoints_remaining
    }

    /// Number of discovery scripts consumed so far.
    ///
    /// Unlike explicit progress, discovery has no total or remaining count
    /// because discovery iterators are unbounded.
    pub fn discovery_consumed(&self) -> usize {
        self.discovery_consumed
    }
}

type InspectScan<K, I> = dyn FnMut(ScanItem<K, I>, ScanProgress) + Send + 'static;

struct CountedQueue<T> {
    items: VecDeque<T>,
    consumed: usize,
}

impl<T> Default for CountedQueue<T> {
    fn default() -> Self {
        Self {
            items: Default::default(),
            consumed: 0,
        }
    }
}

impl<T> CountedQueue<T> {
    fn extend(&mut self, items: impl IntoIterator<Item = T>) {
        self.items.extend(items);
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn consumed(&self) -> usize {
        self.consumed
    }

    fn pop_front(&mut self) -> Option<T> {
        let item = self.items.pop_front()?;
        self.consumed += 1;
        Some(item)
    }
}

struct DiscoveryState<K> {
    stop_gap: usize,
    spks_by_keychain: BTreeMap<K, Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
    consumed: usize,
}

impl<K> Default for DiscoveryState<K> {
    fn default() -> Self {
        Self {
            stop_gap: DEFAULT_STOP_GAP,
            spks_by_keychain: Default::default(),
            consumed: 0,
        }
    }
}

struct ExplicitSyncState<I> {
    spks: CountedQueue<(I, ScriptBuf)>,
    spk_expected_txids: HashMap<ScriptBuf, HashSet<Txid>>,
    txids: CountedQueue<Txid>,
    outpoints: CountedQueue<OutPoint>,
}

impl<I> Default for ExplicitSyncState<I> {
    fn default() -> Self {
        Self {
            spks: Default::default(),
            spk_expected_txids: Default::default(),
            txids: Default::default(),
            outpoints: Default::default(),
        }
    }
}

/// Builds a [`ScanRequest`].
///
/// Construct with [`ScanRequest::builder`].
#[must_use]
pub struct ScanRequestBuilder<K, I = (), D = BlockHash> {
    inner: ScanRequest<K, I, D>,
}

impl<K: Ord, I, D> ScanRequestBuilder<K, I, D> {
    /// Set the stop gap for keychain discovery. Default is 20.
    ///
    /// The stop gap controls how many consecutive unused scripts must be found before
    /// stopping discovery for a keychain. It applies only to the discovery portion of a
    /// [`ScanRequest`] and has no effect when no keychains are added with
    /// [`discover_keychain`](Self::discover_keychain).
    pub fn stop_gap(mut self, stop_gap: usize) -> Self {
        self.inner.discovery.stop_gap = stop_gap;
        self
    }

    /// Add a keychain for discovery.
    ///
    /// The `spks` iterator provides scripts to scan for the given `keychain`. Discovery
    /// continues until `stop_gap` consecutive unused scripts are found.
    pub fn discover_keychain(
        mut self,
        keychain: K,
        spks: impl IntoIterator<IntoIter = impl Iterator<Item = Indexed<ScriptBuf>> + Send + 'static>,
    ) -> Self {
        self.inner
            .discovery
            .spks_by_keychain
            .insert(keychain, Box::new(spks.into_iter()));
        self
    }
}

impl<K, D> ScanRequestBuilder<K, (), D> {
    /// Add [`Script`]s to scan.
    pub fn spks(self, spks: impl IntoIterator<Item = ScriptBuf>) -> Self {
        self.spks_with_indexes(spks.into_iter().map(|spk| ((), spk)))
    }
}

impl<K, I, D> ScanRequestBuilder<K, I, D> {
    /// Set the initial chain tip for the scan request.
    ///
    /// This is used to update [`LocalChain`](../../bdk_chain/local_chain/struct.LocalChain.html).
    pub fn chain_tip(mut self, cp: CheckPoint<D>) -> Self {
        self.inner.chain_tip = Some(cp);
        self
    }

    /// Add [`Script`]s with associated indexes to scan.
    ///
    /// # Example
    ///
    /// Sync revealed script pubkeys obtained from a
    /// [`KeychainTxOutIndex`](https://docs.rs/bdk_chain/latest/bdk_chain/indexer/keychain_txout/struct.KeychainTxOutIndex.html).
    ///
    /// ```rust
    /// # use bdk_chain::bitcoin::BlockHash;
    /// # use bdk_chain::spk_client::ScanRequest;
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
    /// // Reveal spks for "descriptor_a", then build a scan request. Each spk will be indexed with
    /// // `u32`, which represents the derivation index of the associated spk from "descriptor_a".
    /// let (newly_revealed_spks, _changeset) = indexer
    ///     .reveal_to_target("descriptor_a", 21)
    ///     .expect("keychain must exist");
    /// let _request: ScanRequest<(), u32, BlockHash> = ScanRequest::builder()
    ///     .spks_with_indexes(newly_revealed_spks)
    ///     .build();
    ///
    /// // Sync all revealed spks in the indexer. This time, spks may be derived from different
    /// // keychains. Each spk will be indexed with `(&str, u32)` where `&str` is the keychain
    /// // identifier and `u32` is the derivation index.
    /// let all_revealed_spks = indexer.revealed_spks(..);
    /// let _request: ScanRequest<(), (&str, u32), BlockHash> = ScanRequest::builder()
    ///     .spks_with_indexes(all_revealed_spks)
    ///     .build();
    /// # Ok::<_, bdk_chain::keychain_txout::InsertDescriptorError<_>>(())
    /// ```
    pub fn spks_with_indexes(mut self, spks: impl IntoIterator<Item = (I, ScriptBuf)>) -> Self {
        self.inner.explicit.spks.extend(spks);
        self
    }

    /// Add transactions that are expected to exist under the given spks.
    ///
    /// This is useful for detecting a malicious replacement of an incoming transaction.
    /// It only affects the explicit script queue added with `spks` or
    /// [`spks_with_indexes`](Self::spks_with_indexes). It does not affect discovery scripts added
    /// with [`discover_keychain`](Self::discover_keychain).
    pub fn expected_spk_txids(mut self, txs: impl IntoIterator<Item = (ScriptBuf, Txid)>) -> Self {
        for (spk, txid) in txs {
            self.inner
                .explicit
                .spk_expected_txids
                .entry(spk)
                .or_default()
                .insert(txid);
        }
        self
    }

    /// Add [`Txid`]s to scan.
    pub fn txids(mut self, txids: impl IntoIterator<Item = Txid>) -> Self {
        self.inner.explicit.txids.extend(txids);
        self
    }

    /// Add [`OutPoint`]s to scan.
    pub fn outpoints(mut self, outpoints: impl IntoIterator<Item = OutPoint>) -> Self {
        self.inner.explicit.outpoints.extend(outpoints);
        self
    }

    /// Set the closure that will inspect every scan item visited.
    pub fn inspect<F>(mut self, inspect: F) -> Self
    where
        F: FnMut(ScanItem<K, I>, ScanProgress) + Send + 'static,
    {
        self.inner.inspect = Box::new(inspect);
        self
    }

    /// Build the [`ScanRequest`].
    pub fn build(self) -> ScanRequest<K, I, D> {
        self.inner
    }
}

/// Data required to perform a spk-based blockchain client scan.
///
/// A client scan combines keychain discovery with scanning known scripts, transaction ids, and
/// outpoints. During discovery, the client iterates over the scripts for each keychain and stops
/// after encountering `stop_gap` consecutive scripts with no relevant history. The explicit
/// portion scans a known set of scripts, transaction ids, and outpoints for relevant chain data.
/// The scan can also update the chain from the given [`chain_tip`](ScanRequestBuilder::chain_tip),
/// if provided.
///
/// # Example
///
/// ```rust
/// # use bdk_core::spk_client::ScanRequest;
/// # use bdk_chain::{bitcoin::hashes::Hash, local_chain::LocalChain};
/// # use bdk_core::bitcoin::ScriptBuf;
/// # let (local_chain, _) = LocalChain::from_genesis(Hash::all_zeros());
/// # let discovery_spk = ScriptBuf::default();
/// # let explicit_spk = ScriptBuf::default();
/// // Build a scan request with both keychain discovery and explicit scripts.
/// let _scan_request: ScanRequest = ScanRequest::builder()
///     .chain_tip(local_chain.tip())
///     .stop_gap(2)
///     .discover_keychain((), [(0, discovery_spk)])
///     .spks([explicit_spk])
///     .build();
/// ```
#[must_use]
pub struct ScanRequest<K = (), I = (), D = BlockHash> {
    start_time: u64,
    chain_tip: Option<CheckPoint<D>>,
    discovery: DiscoveryState<K>,
    explicit: ExplicitSyncState<I>,
    inspect: Box<InspectScan<K, I>>,
}

impl<K, I, D> From<ScanRequestBuilder<K, I, D>> for ScanRequest<K, I, D> {
    fn from(builder: ScanRequestBuilder<K, I, D>) -> Self {
        builder.inner
    }
}

impl<K: Ord + Clone, I, D> ScanRequest<K, I, D> {
    /// Start building a [`ScanRequest`] with a given `start_time`.
    ///
    /// `start_time` specifies the start time of scan. Chain sources can use this value to set
    /// [`TxUpdate::seen_ats`](crate::TxUpdate::seen_ats) for mempool transactions. A transaction
    /// without any `seen_ats` is assumed to be unseen in the mempool.
    ///
    /// Use [`ScanRequest::builder`] to use the current timestamp as `start_time` (this requires
    /// `feature = "std"`).
    pub fn builder_at(start_time: u64) -> ScanRequestBuilder<K, I, D> {
        ScanRequestBuilder {
            inner: Self {
                start_time,
                chain_tip: None,
                discovery: Default::default(),
                explicit: Default::default(),
                inspect: Box::new(|_, _| ()),
            },
        }
    }

    /// Start building a [`ScanRequest`] with the current timestamp as the `start_time`.
    ///
    /// Use [`ScanRequest::builder_at`] to manually set the `start_time`, or if `feature = "std"`
    /// is not available.
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    pub fn builder() -> ScanRequestBuilder<K, I, D> {
        let start_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("failed to get current timestamp")
            .as_secs();
        Self::builder_at(start_time)
    }

    /// When the scan request was initiated.
    pub fn start_time(&self) -> u64 {
        self.start_time
    }

    /// Get the chain tip [`CheckPoint`] of this request (if any).
    pub fn chain_tip(&self) -> Option<CheckPoint<D>> {
        self.chain_tip.clone()
    }

    /// Get the stop gap for keychain discovery.
    pub fn stop_gap(&self) -> usize {
        self.discovery.stop_gap
    }

    /// List all keychains registered for discovery.
    pub fn keychains(&self) -> Vec<K> {
        self.discovery.spks_by_keychain.keys().cloned().collect()
    }

    /// Advances the scan request and returns the next discovered indexed [`ScriptBuf`] of the
    /// given `keychain`.
    ///
    /// Returns [`None`] when there are no more scripts for the keychain.
    pub fn next_discovery_spk(&mut self, keychain: K) -> Option<Indexed<ScriptBuf>> {
        self.iter_discovery_spks(keychain).next()
    }

    /// Iterate over discovered indexed [`ScriptBuf`]s for the given `keychain`.
    pub fn iter_discovery_spks(
        &mut self,
        keychain: K,
    ) -> impl Iterator<Item = Indexed<ScriptBuf>> + '_ {
        let spks_consumed = self.explicit.spks.consumed();
        let spks_remaining = self.explicit.spks.len();
        let txids_consumed = self.explicit.txids.consumed();
        let txids_remaining = self.explicit.txids.len();
        let outpoints_consumed = self.explicit.outpoints.consumed();
        let outpoints_remaining = self.explicit.outpoints.len();
        let spks = self.discovery.spks_by_keychain.get_mut(&keychain);
        DiscoveryKeychainSpkIter {
            keychain,
            spks,
            inspect: &mut self.inspect,
            discovery_consumed: &mut self.discovery.consumed,
            spks_consumed,
            spks_remaining,
            txids_consumed,
            txids_remaining,
            outpoints_consumed,
            outpoints_remaining,
        }
    }

    /// Get the [`ScanProgress`] of this request.
    pub fn progress(&self) -> ScanProgress {
        ScanProgress {
            discovery_consumed: self.discovery.consumed,
            spks_consumed: self.explicit.spks.consumed(),
            spks_remaining: self.explicit.spks.len(),
            txids_consumed: self.explicit.txids.consumed(),
            txids_remaining: self.explicit.txids.len(),
            outpoints_consumed: self.explicit.outpoints.consumed(),
            outpoints_remaining: self.explicit.outpoints.len(),
        }
    }

    /// Advances the scan request and returns the next [`ScriptBuf`] with corresponding [`Txid`]
    /// history from the explicit sync queue.
    ///
    /// Returns [`None`] when there are no more scripts remaining.
    pub fn next_spk_with_expected_txids(&mut self) -> Option<SpkWithExpectedTxids> {
        let (i, next_spk) = self.explicit.spks.pop_front()?;
        self._call_inspect(ScanItem::Spk(i, next_spk.as_script()));
        let spk_history = self
            .explicit
            .spk_expected_txids
            .get(&next_spk)
            .cloned()
            .unwrap_or_default();
        Some(SpkWithExpectedTxids {
            spk: next_spk,
            expected_txids: spk_history,
        })
    }

    /// Advances the scan request and returns the next [`Txid`].
    ///
    /// Returns [`None`] when there are no more txids remaining.
    pub fn next_txid(&mut self) -> Option<Txid> {
        let txid = self.explicit.txids.pop_front()?;
        self._call_inspect(ScanItem::Txid(txid));
        Some(txid)
    }

    /// Advances the scan request and returns the next [`OutPoint`].
    ///
    /// Returns [`None`] when there are no more outpoints remaining.
    pub fn next_outpoint(&mut self) -> Option<OutPoint> {
        let outpoint = self.explicit.outpoints.pop_front()?;
        self._call_inspect(ScanItem::OutPoint(outpoint));
        Some(outpoint)
    }

    /// Iterate over [`ScriptBuf`]s with corresponding [`Txid`] histories in the explicit sync
    /// queue.
    pub fn iter_spks_with_expected_txids(
        &mut self,
    ) -> impl ExactSizeIterator<Item = SpkWithExpectedTxids> + '_ {
        ExplicitIter::<K, I, D, SpkWithExpectedTxids>::new(self)
    }

    /// Iterate over [`Txid`]s in the explicit sync queue.
    pub fn iter_txids(&mut self) -> impl ExactSizeIterator<Item = Txid> + '_ {
        ExplicitIter::<K, I, D, Txid>::new(self)
    }

    /// Iterate over [`OutPoint`]s in the explicit sync queue.
    pub fn iter_outpoints(&mut self) -> impl ExactSizeIterator<Item = OutPoint> + '_ {
        ExplicitIter::<K, I, D, OutPoint>::new(self)
    }

    fn _call_inspect(&mut self, item: ScanItem<K, I>) {
        let progress = self.progress();
        (*self.inspect)(item, progress);
    }
}

/// Data returned from a unified spk-based blockchain client scan.
///
/// See also [`ScanRequest`].
#[must_use]
#[derive(Debug)]
pub struct ScanResponse<K, A = ConfirmationBlockTime, D = BlockHash> {
    /// Relevant transaction data discovered during the scan.
    pub tx_update: crate::TxUpdate<A>,
    /// Last active indices per keychain (`K`) found during discovery. An index is active if its
    /// script pubkey had an associated transaction. Explicit-sync hits are not included.
    pub last_active_indices: BTreeMap<K, u32>,
    /// Changes to the chain discovered during the scan.
    pub chain_update: Option<CheckPoint<D>>,
}

impl<K, A, D> Default for ScanResponse<K, A, D> {
    fn default() -> Self {
        Self {
            tx_update: Default::default(),
            last_active_indices: Default::default(),
            chain_update: Default::default(),
        }
    }
}

impl<K, A> ScanResponse<K, A> {
    /// Returns true if the `ScanResponse` is empty.
    pub fn is_empty(&self) -> bool {
        self.tx_update.is_empty()
            && self.last_active_indices.is_empty()
            && self.chain_update.is_none()
    }
}

/// Iterator over discovery scripts for a single keychain.
struct DiscoveryKeychainSpkIter<'r, K, I> {
    keychain: K,
    spks: Option<&'r mut Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
    inspect: &'r mut Box<InspectScan<K, I>>,
    discovery_consumed: &'r mut usize,
    spks_consumed: usize,
    spks_remaining: usize,
    txids_consumed: usize,
    txids_remaining: usize,
    outpoints_consumed: usize,
    outpoints_remaining: usize,
}

impl<K: Clone, I> Iterator for DiscoveryKeychainSpkIter<'_, K, I> {
    type Item = Indexed<ScriptBuf>;

    fn next(&mut self) -> Option<Self::Item> {
        let (i, spk) = self.spks.as_mut()?.next()?;
        *self.discovery_consumed += 1;
        let progress = ScanProgress {
            discovery_consumed: *self.discovery_consumed,
            spks_consumed: self.spks_consumed,
            spks_remaining: self.spks_remaining,
            txids_consumed: self.txids_consumed,
            txids_remaining: self.txids_remaining,
            outpoints_consumed: self.outpoints_consumed,
            outpoints_remaining: self.outpoints_remaining,
        };
        (*self.inspect)(
            ScanItem::Discovery(self.keychain.clone(), i, &spk),
            progress,
        );
        Some((i, spk))
    }
}

/// Iterator over explicit scan items.
struct ExplicitIter<'r, K, I, D, Item> {
    request: &'r mut ScanRequest<K, I, D>,
    marker: core::marker::PhantomData<Item>,
}

impl<'r, K, I, D, Item> ExplicitIter<'r, K, I, D, Item> {
    fn new(request: &'r mut ScanRequest<K, I, D>) -> Self {
        Self {
            request,
            marker: core::marker::PhantomData,
        }
    }
}

impl<'r, K, I, D, Item> ExactSizeIterator for ExplicitIter<'r, K, I, D, Item> where
    ExplicitIter<'r, K, I, D, Item>: Iterator
{
}

impl<K: Ord + Clone, I, D> Iterator for ExplicitIter<'_, K, I, D, SpkWithExpectedTxids> {
    type Item = SpkWithExpectedTxids;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_spk_with_expected_txids()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.explicit.spks.len();
        (remaining, Some(remaining))
    }
}

impl<K: Ord + Clone, I, D> Iterator for ExplicitIter<'_, K, I, D, Txid> {
    type Item = Txid;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_txid()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.explicit.txids.len();
        (remaining, Some(remaining))
    }
}

impl<K: Ord + Clone, I, D> Iterator for ExplicitIter<'_, K, I, D, OutPoint> {
    type Item = OutPoint;

    fn next(&mut self) -> Option<Self::Item> {
        self.request.next_outpoint()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.request.explicit.outpoints.len();
        (remaining, Some(remaining))
    }
}
