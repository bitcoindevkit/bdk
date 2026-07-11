//! Helper types for spk-based blockchain clients.
use crate::{
    alloc::{boxed::Box, collections::VecDeque, vec::Vec},
    collections::{BTreeMap, HashMap, HashSet},
    CheckPoint, ConfirmationBlockTime, Indexed, TxUpdate, TxUpdateCursor,
};
use bitcoin::{BlockHash, OutPoint, Script, ScriptBuf, Txid};

type OnSyncEvent<I, A> = dyn for<'a> FnMut(SyncRequestEvent<'a, I, A>) + Send + 'static;
type OnFullScanEvent<K, A> = dyn for<'a> FnMut(FullScanRequestEvent<'a, K, A>) + Send + 'static;

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

/// Progress of a [`FullScanRequest`] (total unknown until `stop_gap` terminates the scan).
#[derive(Debug, Clone, Default)]
pub struct FullScanProgress {
    /// Script pubkeys consumed across all keychains.
    pub keychain_spks_consumed: usize,
    /// Consecutive unused script pubkeys past the last-revealed index.
    pub consecutive_unused: usize,
    /// Transactions discovered so far.
    pub txs_discovered: usize,
}

/// Events emitted during [`SyncRequest`] execution.
#[derive(Debug)]
pub enum SyncRequestEvent<'a, I, A = ConfirmationBlockTime> {
    /// A sync item is about to be fetched from the chain source.
    ItemStarted(SyncItem<'a, I>, SyncProgress),
    /// Incremental transaction data safe to pass to
    /// [`TxGraph::apply_update`](../../bdk_chain/tx_graph/struct.TxGraph.html#method.
    /// apply_update).
    ///
    /// On Electrum, confirmed transactions may lack anchors until [`AnchorsResolved`].
    PartialUpdate(TxUpdate<A>),
    /// Anchor-only update after Electrum resolves confirmation proofs.
    AnchorsResolved(TxUpdate<A>),
}

/// Events emitted during [`FullScanRequest`] execution.
#[derive(Debug)]
pub enum FullScanRequestEvent<'a, K, A = ConfirmationBlockTime> {
    /// A script pubkey is about to be scanned.
    SpkStarted {
        /// Keychain being scanned.
        keychain: K,
        /// Derivation index of the script pubkey.
        index: u32,
        /// Script pubkey.
        script: &'a Script,
        /// Scan progress so far.
        progress: FullScanProgress,
    },
    /// Incremental transaction data safe to pass to
    /// [`TxGraph::apply_update`](../../bdk_chain/tx_graph/struct.TxGraph.html#method.
    /// apply_update).
    ///
    /// On Electrum, confirmed transactions may lack anchors until [`AnchorsResolved`].
    PartialUpdate(TxUpdate<A>),
    /// Anchor-only update after Electrum resolves confirmation proofs.
    AnchorsResolved(TxUpdate<A>),
}

/// Backend-facing sink for emitting [`SyncRequestEvent`]s.
///
/// Constructed via [`SyncRequest::event_sink`]. Intended for official chain sources.
pub struct SyncRequestEventSink<'a, I, D = BlockHash> {
    request: &'a mut SyncRequest<I, D>,
}

impl<'a, I, D> SyncRequestEventSink<'a, I, D> {
    /// Emit a [`SyncRequestEvent::PartialUpdate`] if `update` is non-empty.
    pub fn emit_partial_update(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        if !update.is_empty() {
            (self.request.on_event)(SyncRequestEvent::PartialUpdate(update));
        }
    }

    /// Emit a [`SyncRequestEvent::AnchorsResolved`] if `update` is non-empty.
    pub fn emit_anchors_resolved(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        if !update.is_empty() {
            (self.request.on_event)(SyncRequestEvent::AnchorsResolved(update));
        }
    }
}

/// Backend-facing sink for emitting [`FullScanRequestEvent`]s.
///
/// Constructed via [`FullScanRequest::event_sink`]. Intended for official chain sources.
pub struct FullScanRequestEventSink<'a, K, D = BlockHash> {
    request: &'a mut FullScanRequest<K, D>,
}

impl<'a, K: Ord + Clone, D> FullScanRequestEventSink<'a, K, D> {
    /// Emit a [`FullScanRequestEvent::PartialUpdate`] if `update` is non-empty.
    pub fn emit_partial_update(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        if !update.is_empty() {
            (self.request.on_event)(FullScanRequestEvent::PartialUpdate(update));
        }
    }

    /// Emit a [`FullScanRequestEvent::AnchorsResolved`] if `update` is non-empty.
    pub fn emit_anchors_resolved(&mut self, update: TxUpdate<ConfirmationBlockTime>) {
        if !update.is_empty() {
            (self.request.on_event)(FullScanRequestEvent::AnchorsResolved(update));
        }
    }

    /// Update full-scan progress counters (called by chain sources between batches).
    pub fn set_full_scan_progress(&mut self, progress: FullScanProgress) {
        self.request.full_scan_progress = progress;
    }

    /// Access the current full-scan progress.
    pub fn full_scan_progress(&self) -> &FullScanProgress {
        &self.request.full_scan_progress
    }

    /// Mutable access to full-scan progress.
    pub fn full_scan_progress_mut(&mut self) -> &mut FullScanProgress {
        &mut self.request.full_scan_progress
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
    /// Sync revealed script pubkeys obtained from a
    /// [`KeychainTxOutIndex`](https://docs.rs/bdk_chain/latest/bdk_chain/indexer/keychain_txout/struct.KeychainTxOutIndex.html).
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

    /// Register a callback for sync events including progress and partial transaction updates.
    ///
    /// When partial updates are applied during sync, [`SyncResponse::tx_update`] at the end
    /// contains only undrained remainder (empty when everything was streamed).
    pub fn on_event<F>(mut self, on_event: F) -> Self
    where
        F: for<'a> FnMut(SyncRequestEvent<'a, I>) + Send + 'static,
    {
        self.inner.emit_partial_updates = true;
        self.inner.on_event = Box::new(on_event);
        self
    }

    /// Set the closure that will inspect every sync item visited.
    ///
    /// This is sugar over [`Self::on_event`] for progress-only callbacks.
    pub fn inspect<F>(self, mut inspect: F) -> Self
    where
        F: FnMut(SyncItem<I>, SyncProgress) + Send + 'static,
    {
        self.on_event(move |event| {
            if let SyncRequestEvent::ItemStarted(item, progress) = event {
                inspect(item, progress);
            }
        })
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
    on_event: Box<OnSyncEvent<I, ConfirmationBlockTime>>,
    /// Whether to emit partial [`TxUpdate`]s during sync (set by
    /// [`SyncRequestBuilder::on_event`]).
    pub emit_partial_updates: bool,
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
                on_event: Box::new(|_| ()),
                emit_partial_updates: false,
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

    /// Returns a sink for emitting sync events from chain sources.
    pub fn event_sink(&mut self) -> SyncRequestEventSink<'_, I, D> {
        SyncRequestEventSink { request: self }
    }

    /// Emit a partial [`TxUpdate`] if [`Self::emit_partial_updates`] is enabled.
    pub fn try_emit_partial_update(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) {
        if !self.emit_partial_updates {
            return;
        }
        let cursor = cursor.get_or_insert_with(TxUpdateCursor::new);
        let delta = tx_update.drain_since(cursor);
        if !delta.is_empty() {
            (self.on_event)(SyncRequestEvent::PartialUpdate(delta));
        }
    }

    /// Emit an anchor-only [`TxUpdate`] if [`Self::emit_partial_updates`] is enabled.
    pub fn try_emit_anchors_resolved(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) {
        if !self.emit_partial_updates {
            return;
        }
        let cursor = cursor.get_or_insert_with(TxUpdateCursor::new);
        let delta = tx_update.drain_since(cursor);
        if !delta.is_empty() {
            (self.on_event)(SyncRequestEvent::AnchorsResolved(delta));
        }
    }

    fn _call_inspect(&mut self, item: SyncItem<I>) {
        let progress = self.progress();
        (self.on_event)(SyncRequestEvent::ItemStarted(item, progress));
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

    /// Record the last revealed script pubkey `index` for a given `keychain`.
    ///
    /// `full_scan` covers `0..=index` for this keychain; `stop_gap`
    /// applies only to indices past `index`. Keychains without a recorded last revealed
    /// index fall back to applying `stop_gap` from index 0.
    /// Users working with a `KeychainTxOutIndex` usually don't call this directly,
    /// `spks_from_indexer` (from `bdk_chain`) populates it automatically.
    pub fn last_revealed_for_keychain(mut self, keychain: K, index: u32) -> Self {
        self.inner.last_revealed.insert(keychain, index);
        self
    }

    /// Register a callback for full-scan events including progress and partial transaction updates.
    ///
    /// When partial updates are applied during the scan, [`FullScanResponse::tx_update`] at the end
    /// contains only undrained remainder (empty when everything was streamed).
    pub fn on_event<F>(mut self, on_event: F) -> Self
    where
        F: for<'a> FnMut(FullScanRequestEvent<'a, K>) + Send + 'static,
    {
        self.inner.emit_partial_updates = true;
        self.inner.on_event = Box::new(on_event);
        self
    }

    /// Set the closure that will inspect every sync item visited.
    ///
    /// This is sugar over [`Self::on_event`] for progress-only callbacks.
    pub fn inspect<F>(self, mut inspect: F) -> Self
    where
        F: FnMut(K, u32, &Script) + Send + 'static,
    {
        self.on_event(move |event| {
            if let FullScanRequestEvent::SpkStarted {
                keychain,
                index,
                script,
                ..
            } = event
            {
                inspect(keychain, index, script);
            }
        })
    }

    /// Build the [`FullScanRequest`].
    pub fn build(self) -> FullScanRequest<K, D> {
        self.inner
    }
}

/// Data required to perform a spk-based blockchain client full scan.
///
/// A client full scan iterates through all the scripts for the given keychains, fetching relevant
/// data. It always scans the revealed range (up to the last-revealed index), then keeps going until
/// a run of `stop_gap` consecutive scripts with no data is found. This operation is generally only
/// used when importing or restoring previously used keychains in which the list of used scripts is
/// not known. The full scan process also updates the chain from the given
/// [`chain_tip`](FullScanRequestBuilder::chain_tip) (if provided).
#[must_use]
pub struct FullScanRequest<K, D = BlockHash> {
    start_time: u64,
    chain_tip: Option<CheckPoint<D>>,
    spks_by_keychain: BTreeMap<K, Box<dyn Iterator<Item = Indexed<ScriptBuf>> + Send>>,
    last_revealed: BTreeMap<K, u32>,
    full_scan_progress: FullScanProgress,
    on_event: Box<OnFullScanEvent<K, ConfirmationBlockTime>>,
    /// Whether to emit partial [`TxUpdate`]s during full scan (set by
    /// [`FullScanRequestBuilder::on_event`]).
    pub emit_partial_updates: bool,
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
                last_revealed: BTreeMap::new(),
                full_scan_progress: FullScanProgress::default(),
                on_event: Box::new(|_| ()),
                emit_partial_updates: false,
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

    /// Get the last revealed script pubkey index for `keychain` (if set).
    ///
    /// Chain sources use this to scan `0..=last_revealed` before applying
    /// `stop_gap` to further discovery.
    pub fn last_revealed(&self, keychain: &K) -> Option<u32> {
        self.last_revealed.get(keychain).copied()
    }

    /// Advances the full scan request and returns the next indexed [`ScriptBuf`] of the given
    /// `keychain`.
    pub fn next_spk(&mut self, keychain: K) -> Option<Indexed<ScriptBuf>> {
        self.iter_spks(keychain).next()
    }

    /// Iterate over indexed [`ScriptBuf`]s contained in this request of the given `keychain`.
    pub fn iter_spks(&mut self, keychain: K) -> impl Iterator<Item = Indexed<ScriptBuf>> + '_ {
        KeychainSpkIter {
            keychain: keychain.clone(),
            spks: self.spks_by_keychain.get_mut(&keychain),
            full_scan_progress: &mut self.full_scan_progress,
            on_event: &mut self.on_event,
        }
    }

    /// Returns a sink for emitting full-scan events from chain sources.
    pub fn event_sink(&mut self) -> FullScanRequestEventSink<'_, K, D> {
        FullScanRequestEventSink { request: self }
    }

    /// Emit a partial [`TxUpdate`] if [`Self::emit_partial_updates`] is enabled.
    pub fn try_emit_partial_update(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) {
        if !self.emit_partial_updates {
            return;
        }
        let cursor = cursor.get_or_insert_with(TxUpdateCursor::new);
        let delta = tx_update.drain_since(cursor);
        if !delta.is_empty() {
            (self.on_event)(FullScanRequestEvent::PartialUpdate(delta));
        }
    }

    /// Emit an anchor-only [`TxUpdate`] if [`Self::emit_partial_updates`] is enabled.
    pub fn try_emit_anchors_resolved(
        &mut self,
        cursor: &mut Option<TxUpdateCursor<ConfirmationBlockTime>>,
        tx_update: &mut TxUpdate<ConfirmationBlockTime>,
    ) {
        if !self.emit_partial_updates {
            return;
        }
        let cursor = cursor.get_or_insert_with(TxUpdateCursor::new);
        let delta = tx_update.drain_since(cursor);
        if !delta.is_empty() {
            (self.on_event)(FullScanRequestEvent::AnchorsResolved(delta));
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
    full_scan_progress: &'r mut FullScanProgress,
    on_event: &'r mut Box<OnFullScanEvent<K, ConfirmationBlockTime>>,
}

impl<K: Ord + Clone> Iterator for KeychainSpkIter<'_, K> {
    type Item = Indexed<ScriptBuf>;

    fn next(&mut self) -> Option<Self::Item> {
        let (i, spk) = self.spks.as_mut()?.next()?;
        self.full_scan_progress.keychain_spks_consumed = self
            .full_scan_progress
            .keychain_spks_consumed
            .saturating_add(1);
        let progress = self.full_scan_progress.clone();
        (self.on_event)(FullScanRequestEvent::SpkStarted {
            keychain: self.keychain.clone(),
            index: i,
            script: spk.as_script(),
            progress,
        });
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
