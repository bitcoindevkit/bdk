use crate::collections::BTreeMap;
use crate::local_chain::CheckPoint;
use crate::{local_chain, ConfirmationTimeHeightAnchor, TxGraph};
use alloc::{boxed::Box, vec::Vec};
use bitcoin::{OutPoint, ScriptBuf, Txid};
use core::default::Default;
use core::fmt::Debug;
use std::sync::Arc;

/// Helper types for use with spk-based blockchain clients.

type InspectSpkFn = Arc<Box<dyn Fn(u32, &ScriptBuf) + Send + Sync + 'static>>;
type InspectKeychainSpkFn<K> = Arc<Box<dyn Fn(K, u32, &ScriptBuf) + Send + Sync + 'static>>;
type InspectTxidFn = Arc<Box<dyn Fn(&Txid) + Send + Sync + 'static>>;
type InspectOutPointFn = Arc<Box<dyn Fn(&OutPoint) + Send + Sync + 'static>>;

/// Data required to perform a spk-based blockchain client sync.
///
/// A client sync fetches relevant chain data for a known list of scripts, transaction ids and
/// outpoints. The sync process also updates the chain from the given [`CheckPoint`].
pub struct SyncRequest {
    /// A checkpoint for the current chain [`LocalChain::tip`].
    /// The sync process will return a new chain update that extends this tip.
    ///
    /// [`LocalChain::tip`]: local_chain::LocalChain::tip
    pub chain_tip: CheckPoint,
    /// Transactions that spend from or to these indexed script pubkeys.
    spks: Vec<(u32, ScriptBuf)>,
    /// Transactions with these txids.
    txids: Vec<Txid>,
    /// Transactions with these outpoints or spend from these outpoints.
    outpoints: Vec<OutPoint>,
    /// An optional call-back function to inspect sync'd spks
    inspect_spks: Option<InspectSpkFn>,
    /// An optional call-back function to inspect sync'd txids
    inspect_txids: Option<InspectTxidFn>,
    /// An optional call-back function to inspect sync'd outpoints
    inspect_outpoints: Option<InspectOutPointFn>,
}

fn null_inspect_spks(_index: u32, _spk: &ScriptBuf) {}
fn null_inspect_keychain_spks<K>(_keychain: K, _index: u32, _spk: &ScriptBuf) {}
fn null_inspect_txids(_txid: &Txid) {}
fn null_inspect_outpoints(_outpoint: &OutPoint) {}

impl SyncRequest {
    /// Create a new [`SyncRequest`] from the current chain tip [`CheckPoint`].
    pub fn new(chain_tip: CheckPoint) -> Self {
        Self {
            chain_tip,
            spks: Default::default(),
            txids: Default::default(),
            outpoints: Default::default(),
            inspect_spks: Default::default(),
            inspect_txids: Default::default(),
            inspect_outpoints: Default::default(),
        }
    }

    /// Add [`ScriptBuf`]s to be sync'd with this request.
    pub fn add_spks(&mut self, spks: impl IntoIterator<Item = (u32, ScriptBuf)>) {
        self.spks.extend(spks.into_iter())
    }

    /// Take the [`ScriptBuf`]s to be sync'd with this request.
    pub fn take_spks(&mut self) -> impl Iterator<Item = (u32, ScriptBuf)> {
        let spks = core::mem::take(&mut self.spks);
        let inspect = self
            .inspect_spks
            .take()
            .unwrap_or(Arc::new(Box::new(null_inspect_spks)));
        spks.into_iter()
            .inspect(move |(index, spk)| inspect(*index, spk))
    }

    /// Add a function that will be called for each [`ScriptBuf`] sync'd in this request.
    pub fn inspect_spks(&mut self, inspect: impl Fn(u32, &ScriptBuf) + Send + Sync + 'static) {
        self.inspect_spks = Some(Arc::new(Box::new(inspect)))
    }

    /// Add [`Txid`]s to be sync'd with this request.
    pub fn add_txids(&mut self, txids: impl IntoIterator<Item = Txid>) {
        self.txids.extend(txids.into_iter())
    }

    /// Take the [`Txid`]s to be sync'd with this request.
    pub fn take_txids(&mut self) -> impl Iterator<Item = Txid> {
        let txids = core::mem::take(&mut self.txids);
        let inspect = self
            .inspect_txids
            .clone()
            .unwrap_or(Arc::new(Box::new(null_inspect_txids)));
        txids.into_iter().inspect(move |t| inspect(t))
    }

    /// Add a function that will be called for each [`Txid`] sync'd in this request.
    pub fn inspect_txids(&mut self, inspect: impl Fn(&Txid) + Send + Sync + 'static) {
        self.inspect_txids = Some(Arc::new(Box::new(inspect)))
    }

    /// Add [`OutPoint`]s to be sync'd with this request.
    pub fn add_outpoints(&mut self, outpoints: impl IntoIterator<Item = OutPoint>) {
        self.outpoints.extend(outpoints.into_iter())
    }

    /// Take the [`OutPoint`]s to be sync'd with this request.
    pub fn take_outpoints(&mut self) -> impl Iterator<Item = OutPoint> {
        let outpoints = core::mem::take(&mut self.outpoints);
        let inspect = self
            .inspect_outpoints
            .take()
            .unwrap_or(Arc::new(Box::new(null_inspect_outpoints)));
        outpoints.into_iter().inspect(move |o| inspect(o))
    }

    /// Add a function that will be called for each [`OutPoint`] sync'd in this request.
    pub fn inspect_outpoints(&mut self, inspect: impl Fn(&OutPoint) + Send + Sync + 'static) {
        self.inspect_outpoints = Some(Arc::new(Box::new(inspect)))
    }
}

/// Data returned from a spk-based blockchain client sync.
///
/// See also [`SyncRequest`].
pub struct SyncResult {
    /// The update to apply to the receiving [`TxGraph`].
    pub graph_update: TxGraph<ConfirmationTimeHeightAnchor>,
    /// The update to apply to the receiving [`LocalChain`](local_chain::LocalChain).
    pub chain_update: local_chain::Update,
}

/// Data required to perform a spk-based blockchain client full scan.
///
/// A client full scan iterates through all the scripts for the given keychains, fetching relevant
/// data until some stop gap number of scripts is found that have no data. This operation is
/// generally only used when importing or restoring previously used keychains in which the list of
/// used scripts is not known. The full scan process also updates the chain from the given [`CheckPoint`].
pub struct FullScanRequest<K, I> {
    /// A checkpoint for the current [`LocalChain::tip`].
    /// The full scan process will return a new chain update that extends this tip.
    ///
    /// [`LocalChain::tip`]: local_chain::LocalChain::tip
    pub chain_tip: CheckPoint,
    /// Iterators of script pubkeys indexed by the keychain index.
    spks_by_keychain: BTreeMap<K, I>,
    /// An optional call-back function to inspect scanned spks
    inspect_spks: Option<InspectKeychainSpkFn<K>>,
}

/// Create a new [`FullScanRequest`] from the current chain tip [`CheckPoint`].
impl<K: Ord + Clone + Send + Debug + 'static, I: Iterator<Item = (u32, ScriptBuf)> + Send>
    FullScanRequest<K, I>
{
    /// Create a new [`FullScanRequest`] from the current chain tip [`CheckPoint`].
    pub fn new(chain_tip: CheckPoint) -> Self {
        Self {
            chain_tip,
            spks_by_keychain: Default::default(),
            inspect_spks: Default::default(),
        }
    }

    /// Add map of keychain's to tuple of index, [`ScriptBuf`] iterators to be scanned with this
    /// request.
    ///
    /// Adding a map with a keychain that has already been added will overwrite the previously added
    /// keychain [`ScriptBuf`] iterator.
    pub fn add_spks_by_keychain(&mut self, spks_by_keychain: BTreeMap<K, I>) {
        self.spks_by_keychain.extend(spks_by_keychain)
    }

    /// Take the map of keychain, [`ScriptBuf`]s to be full scanned with this request.
    pub fn take_spks_by_keychain(
        &mut self,
    ) -> BTreeMap<
        K,
        Box<impl IntoIterator<IntoIter = impl Iterator<Item = (u32, ScriptBuf)> + Send> + Send>,
    > {
        let spks = core::mem::take(&mut self.spks_by_keychain);
        let inspect = self
            .inspect_spks
            .clone()
            .unwrap_or(Arc::new(Box::new(null_inspect_keychain_spks)));

        spks.into_iter()
            .map(move |(k, spk_iter)| {
                let inspect = inspect.clone();
                let keychain = k.clone();
                let spk_iter_inspected =
                    Box::new(spk_iter.inspect(move |(i, spk)| inspect(keychain.clone(), *i, spk)));
                (k, spk_iter_inspected)
            })
            .collect()
    }

    /// Add a function that will be called for each [`ScriptBuf`] sync'd in this request.
    pub fn inspect_spks(&mut self, inspect: impl Fn(K, u32, &ScriptBuf) + Send + Sync + 'static) {
        self.inspect_spks = Some(Arc::new(Box::new(inspect)))
    }
}

/// Data returned from a spk-based blockchain client full scan.
///
/// See also [`FullScanRequest`].
pub struct FullScanResult<K: Ord + Clone + Send + Debug> {
    /// The update to apply to the receiving [`LocalChain`](local_chain::LocalChain).
    pub graph_update: TxGraph<ConfirmationTimeHeightAnchor>,
    /// The update to apply to the receiving [`TxGraph`].
    pub chain_update: local_chain::Update,
    /// Last active indices for the corresponding keychains (`K`).
    pub last_active_indices: BTreeMap<K, u32>,
}
