use crate::{
    collections::*,
    indexed_tx_graph::Indexer,
    miniscript::{Descriptor, DescriptorPublicKey},
    spk_iter::BIP32_MAX_INDEX,
    SpkIterator, SpkTxOutIndex,
};
use alloc::string::ToString;
use bitcoin::{OutPoint, Script, Transaction, TxOut, Txid};
use core::{
    fmt::Debug,
    ops::{Bound, RangeBounds},
};

use crate::Append;

type DescriptorId = [u8; 8];

/// Represents updates to the derivation index of a [`KeychainTxOutIndex`].
/// It maps each keychain `K` to a descriptor and its last revealed index.
///
/// It can be applied to [`KeychainTxOutIndex`] with [`apply_changeset`]. [`ChangeSet] are
/// monotone in that they will never decrease the revealed derivation index.
///
/// [`KeychainTxOutIndex`]: crate::keychain::KeychainTxOutIndex
/// [`apply_changeset`]: crate::keychain::KeychainTxOutIndex::apply_changeset
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(
        crate = "serde_crate",
        bound(
            deserialize = "K: Ord + serde::Deserialize<'de>",
            serialize = "K: Ord + serde::Serialize"
        )
    )
)]
#[must_use]
pub struct ChangeSet<K> {
    /// Contains the keychains that have been added and their respective descriptor
    pub keychains_added: BTreeMap<K, Descriptor<DescriptorPublicKey>>,
    /// Contains for each descriptor_id the last revealed index of derivation
    pub last_revealed: BTreeMap<DescriptorId, u32>,
}

impl<K: Ord> Append for ChangeSet<K> {
    /// Append another [`ChangeSet`] into self.
    ///
    /// For each keychain in `keychains_added` in the given [`ChangeSet`]:
    /// If the keychain already exist with a different descriptor, we overwrite the old descriptor.
    ///
    /// For each `last_revealed` in the given [`ChangeSet`]:
    /// If the keychain already exists, increase the index when the other's index > self's index.
    fn append(&mut self, mut other: Self) {
        for (keychain, descriptor) in &mut self.keychains_added {
            if let Some(other_descriptor) = other.keychains_added.remove(keychain) {
                *descriptor = other_descriptor;
            }
        }

        for (descriptor_id, index) in &mut self.last_revealed {
            if let Some(other_index) = other.last_revealed.remove(descriptor_id) {
                *index = other_index.max(*index);
            }
        }

        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        self.keychains_added.extend(other.keychains_added);
        self.last_revealed.extend(other.last_revealed);
    }

    /// Returns whether the changeset are empty.
    fn is_empty(&self) -> bool {
        self.last_revealed.is_empty() && self.keychains_added.is_empty()
    }
}

impl<K> Default for ChangeSet<K> {
    fn default() -> Self {
        Self {
            last_revealed: BTreeMap::default(),
            keychains_added: BTreeMap::default(),
        }
    }
}

/// The default script lookahead used when recovering a wallet keychain, see [`KeychainTxOutIndex`].
pub const DEFAULT_LOOKAHEAD: u32 = 25;

/// [`KeychainTxOutIndex`] controls how script pubkeys are revealed for multiple keychains, and
/// indexes [`TxOut`]s with them.
///
/// A single keychain is a chain of script pubkeys derived from a single [`Descriptor`]. Keychains
/// are identified using the `K` generic. Script pubkeys are identified by the keychain that they
/// are derived from `K`, as well as the derivation index `u32`.
///
/// # Revealed script pubkeys
///
/// Tracking how script pubkeys are revealed is useful for collecting chain data. For example, if
/// the user has requested 5 script pubkeys (to receive money with), we only need to use those
/// script pubkeys to scan for chain data.
///
/// Call [`reveal_to_target`] or [`reveal_next_spk`] to reveal more script pubkeys.
/// Call [`revealed_keychain_spks`] or [`revealed_spks`] to iterate through revealed script pubkeys.
///
/// # Lookahead script pubkeys
///
/// When an user first recovers a wallet (i.e. from a recovery phrase and/or descriptor), we will
/// NOT have knowledge of which script pubkeys are revealed. So when we index a transaction or
/// txout (using [`index_tx`]/[`index_txout`]) we scan the txouts against script pubkeys derived
/// above the last revealed index. These additionally-derived script pubkeys are called the
/// lookahead.
///
/// The [`KeychainTxOutIndex`] is constructed with the `lookahead` and cannot be altered. The
/// default `lookahead` count is [`DEFAULT_LOOKAHEAD`]. Use [`new`] to set a custom `lookahead`.
///
/// # Unbounded script pubkey iterator
///
/// For script-pubkey-based chain sources (such as Electrum/Esplora), an initial scan is best done
/// by iterating though derived script pubkeys one by one and requesting transaction histories for
/// each script pubkey. We will stop after x-number of script pubkeys have empty histories. An
/// unbounded script pubkey iterator is useful to pass to such a chain source.
///
/// Call [`unbounded_spk_iter`] to get an unbounded script pubkey iterator for a given keychain.
/// Call [`all_unbounded_spk_iters`] to get unbounded script pubkey iterators for all keychains.
///
/// # Change sets
///
/// Methods that can update the last revealed index or add keychains will return [`super::ChangeSet`] to report
/// these changes. This can be persisted for future recovery.
///
/// ## Synopsis
///
/// ```
/// use bdk_chain::keychain::KeychainTxOutIndex;
/// # use bdk_chain::{ miniscript::{Descriptor, DescriptorPublicKey} };
/// # use core::str::FromStr;
///
/// // imagine our service has internal and external addresses but also addresses for users
/// #[derive(Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
/// enum MyKeychain {
///     External,
///     Internal,
///     MyAppUser {
///         user_id: u32
///     }
/// }
///
/// let mut txout_index = KeychainTxOutIndex::<MyKeychain>::default();
///
/// # let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::signing_only();
/// # let (external_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
/// # let (internal_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)").unwrap();
/// # let (descriptor_for_user_42, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/2/*)").unwrap();
/// txout_index.add_keychain(MyKeychain::External, external_descriptor);
/// txout_index.add_keychain(MyKeychain::Internal, internal_descriptor);
/// txout_index.add_keychain(MyKeychain::MyAppUser { user_id: 42 }, descriptor_for_user_42);
///
/// let new_spk_for_user = txout_index.reveal_next_spk(&MyKeychain::MyAppUser{ user_id: 42 });
/// ```
///
/// [`Ord`]: core::cmp::Ord
/// [`SpkTxOutIndex`]: crate::spk_txout_index::SpkTxOutIndex
/// [`Descriptor`]: crate::miniscript::Descriptor
/// [`reveal_to_target`]: KeychainTxOutIndex::reveal_to_target
/// [`reveal_next_spk`]: KeychainTxOutIndex::reveal_next_spk
/// [`revealed_keychain_spks`]: KeychainTxOutIndex::revealed_keychain_spks
/// [`revealed_spks`]: KeychainTxOutIndex::revealed_spks
/// [`index_tx`]: KeychainTxOutIndex::index_tx
/// [`index_txout`]: KeychainTxOutIndex::index_txout
/// [`new`]: KeychainTxOutIndex::new
/// [`unbounded_spk_iter`]: KeychainTxOutIndex::unbounded_spk_iter
/// [`all_unbounded_spk_iters`]: KeychainTxOutIndex::all_unbounded_spk_iters
#[derive(Clone, Debug)]
pub struct KeychainTxOutIndex<K> {
    inner: SpkTxOutIndex<(DescriptorId, u32)>,
    // keychain -> (descriptor, descriptor id) map
    keychains_to_descriptors: BTreeMap<K, (DescriptorId, Descriptor<DescriptorPublicKey>)>,
    // descriptor id -> keychain map
    descriptor_ids_to_keychain: BTreeMap<DescriptorId, K>,
    // last revealed indexes
    last_revealed: BTreeMap<DescriptorId, u32>,
    // lookahead settings for each keychain
    lookahead: u32,
}

impl<K> Default for KeychainTxOutIndex<K> {
    fn default() -> Self {
        Self::new(DEFAULT_LOOKAHEAD)
    }
}

impl<K: Clone + Ord + Debug> Indexer for KeychainTxOutIndex<K> {
    type ChangeSet = super::ChangeSet<K>;

    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::ChangeSet {
        match self.inner.scan_txout(outpoint, txout).cloned() {
            Some((keychain, index)) => {
                let descriptor_id = self
                    .keychain_of_descriptor_id(&keychain)
                    .expect("Must be here");
                self.reveal_to_target(&descriptor_id.clone(), index).1
            }
            None => super::ChangeSet::default(),
        }
    }

    fn index_tx(&mut self, tx: &bitcoin::Transaction) -> Self::ChangeSet {
        let mut changeset = super::ChangeSet::<K>::default();
        for (op, txout) in tx.output.iter().enumerate() {
            changeset.append(self.index_txout(OutPoint::new(tx.txid(), op as u32), txout));
        }
        changeset
    }

    fn initial_changeset(&self) -> Self::ChangeSet {
        super::ChangeSet {
            keychains_added: self
                .keychains()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            last_revealed: self.last_revealed.clone(),
        }
    }

    fn apply_changeset(&mut self, changeset: Self::ChangeSet) {
        self.apply_changeset(changeset)
    }

    fn is_tx_relevant(&self, tx: &bitcoin::Transaction) -> bool {
        self.inner.is_relevant(tx)
    }
}

impl<K> KeychainTxOutIndex<K> {
    /// Construct a [`KeychainTxOutIndex`] with the given `lookahead`.
    ///
    /// The `lookahead` is the number of script pubkeys to derive and cache from the internal
    /// descriptors over and above the last revealed script index. Without a lookahead the index
    /// will miss outputs you own when processing transactions whose output script pubkeys lie
    /// beyond the last revealed index. In certain situations, such as when performing an initial
    /// scan of the blockchain during wallet import, it may be uncertain or unknown what the index
    /// of the last revealed script pubkey actually is.
    ///
    /// Refer to [struct-level docs](KeychainTxOutIndex) for more about `lookahead`.
    pub fn new(lookahead: u32) -> Self {
        Self {
            inner: SpkTxOutIndex::default(),
            descriptor_ids_to_keychain: BTreeMap::new(),
            keychains_to_descriptors: BTreeMap::new(),
            last_revealed: BTreeMap::new(),
            lookahead,
        }
    }
}

/// Methods that are *re-exposed* from the internal [`SpkTxOutIndex`].
impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Returns the corresponding (descriptor_id, descriptor) given a keychain, if exists
    fn descriptor_of_keychain(
        &self,
        keychain: &K,
    ) -> Option<&(DescriptorId, Descriptor<DescriptorPublicKey>)> {
        self.keychains_to_descriptors.get(keychain)
    }

    /// Returns the corresponding keychain given a descriptor id, if exists
    fn keychain_of_descriptor_id(&self, descriptor_id: &[u8; 8]) -> Option<&K> {
        self.descriptor_ids_to_keychain.get(descriptor_id)
    }

    /// Return a reference to the internal [`SpkTxOutIndex`].
    ///
    /// **WARNING:** The internal index will contain lookahead spks. Refer to
    /// [struct-level docs](KeychainTxOutIndex) for more about `lookahead`.
    pub fn inner(&self) -> &SpkTxOutIndex<(DescriptorId, u32)> {
        &self.inner
    }

    /// Get a reference to the set of indexed outpoints.
    pub fn outpoints(&self) -> BTreeSet<((K, u32), OutPoint)> {
        self.inner
            .outpoints()
            .iter()
            .map(|((desc_id, index), op)| {
                (
                    (
                        self.keychain_of_descriptor_id(desc_id)
                            .expect("Must be here")
                            .clone(),
                        *index,
                    ),
                    *op,
                )
            })
            .collect()
    }

    /// Iterate over known txouts that spend to tracked script pubkeys.
    pub fn txouts(
        &self,
    ) -> impl DoubleEndedIterator<Item = (K, u32, OutPoint, &TxOut)> + ExactSizeIterator {
        self.inner.txouts().map(|((desc_id, i), op, txo)| {
            (
                self.keychain_of_descriptor_id(desc_id)
                    .expect("Must be here")
                    .clone(),
                *i,
                op,
                txo,
            )
        })
    }

    /// Finds all txouts on a transaction that has previously been scanned and indexed.
    pub fn txouts_in_tx(
        &self,
        txid: Txid,
    ) -> impl DoubleEndedIterator<Item = (K, u32, OutPoint, &TxOut)> {
        self.inner
            .txouts_in_tx(txid)
            .map(|((desc_id, i), op, txo)| {
                (
                    self.keychain_of_descriptor_id(desc_id)
                        .expect("Must be here")
                        .clone(),
                    *i,
                    op,
                    txo,
                )
            })
    }

    /// Return the [`TxOut`] of `outpoint` if it has been indexed.
    ///
    /// The associated keychain and keychain index of the txout's spk is also returned.
    ///
    /// This calls [`SpkTxOutIndex::txout`] internally.
    pub fn txout(&self, outpoint: OutPoint) -> Option<(K, u32, &TxOut)> {
        let ((descriptor_id, index), txo) = self.inner.txout(outpoint)?;
        let keychain = self.keychain_of_descriptor_id(descriptor_id)?;
        Some((keychain.clone(), *index, txo))
    }

    /// Return the script that exists under the given `keychain`'s `index`.
    ///
    /// This calls [`SpkTxOutIndex::spk_at_index`] internally.
    pub fn spk_at_index(&self, keychain: K, index: u32) -> Option<&Script> {
        let descriptor_id = self.keychains_to_descriptors.get(&keychain)?.0;
        self.inner.spk_at_index(&(descriptor_id, index))
    }

    /// Returns the keychain and keychain index associated with the spk.
    ///
    /// This calls [`SpkTxOutIndex::index_of_spk`] internally.
    pub fn index_of_spk(&self, script: &Script) -> Option<(K, u32)> {
        self.inner
            .index_of_spk(script)
            .cloned()
            .map(|(desc_id, last_index)| {
                (
                    self.keychain_of_descriptor_id(&desc_id)
                        .expect("must be here")
                        .clone(),
                    last_index,
                )
            })
    }

    /// Returns whether the spk under the `keychain`'s `index` has been used.
    ///
    /// Here, "unused" means that after the script pubkey was stored in the index, the index has
    /// never scanned a transaction output with it.
    ///
    /// This calls [`SpkTxOutIndex::is_used`] internally.
    pub fn is_used(&self, keychain: K, index: u32) -> bool {
        let descriptor_id = self.keychains_to_descriptors.get(&keychain).map(|k| k.0);
        match descriptor_id {
            Some(descriptor_id) => self.inner.is_used(&(descriptor_id, index)),
            None => false,
        }
    }

    /// Marks the script pubkey at `index` as used even though the tracker hasn't seen an output
    /// with it.
    ///
    /// This only has an effect when the `index` had been added to `self` already and was unused.
    ///
    /// Returns whether the `index` was initially present as `unused`.
    ///
    /// This is useful when you want to reserve a script pubkey for something but don't want to add
    /// the transaction output using it to the index yet. Other callers will consider `index` on
    /// `keychain` used until you call [`unmark_used`].
    ///
    /// This calls [`SpkTxOutIndex::mark_used`] internally.
    ///
    /// [`unmark_used`]: Self::unmark_used
    pub fn mark_used(&mut self, keychain: K, index: u32) -> bool {
        let descriptor_id = self.keychains_to_descriptors.get(&keychain).map(|k| k.0);
        match descriptor_id {
            Some(descriptor_id) => self.inner.mark_used(&(descriptor_id, index)),
            None => false,
        }
    }

    /// Undoes the effect of [`mark_used`]. Returns whether the `index` is inserted back into
    /// `unused`.
    ///
    /// Note that if `self` has scanned an output with this script pubkey, then this will have no
    /// effect.
    ///
    /// This calls [`SpkTxOutIndex::unmark_used`] internally.
    ///
    /// [`mark_used`]: Self::mark_used
    pub fn unmark_used(&mut self, keychain: K, index: u32) -> bool {
        let descriptor_id = self.keychains_to_descriptors.get(&keychain).map(|k| k.0);
        match descriptor_id {
            Some(descriptor_id) => self.inner.unmark_used(&(descriptor_id, index)),
            None => false,
        }
    }

    /// Computes total input value going from script pubkeys in the index (sent) and the total output
    /// value going to script pubkeys in the index (received) in `tx`. For the `sent` to be computed
    /// correctly, the output being spent must have already been scanned by the index. Calculating
    /// received just uses the [`Transaction`] outputs directly, so it will be correct even if it has
    /// not been scanned.
    ///
    /// This calls [`SpkTxOutIndex::sent_and_received`] internally.
    pub fn sent_and_received(&self, tx: &Transaction) -> (u64, u64) {
        self.inner.sent_and_received(tx)
    }

    /// Computes the net value that this transaction gives to the script pubkeys in the index and
    /// *takes* from the transaction outputs in the index. Shorthand for calling
    /// [`sent_and_received`] and subtracting sent from received.
    ///
    /// This calls [`SpkTxOutIndex::net_value`] internally.
    ///
    /// [`sent_and_received`]: Self::sent_and_received
    pub fn net_value(&self, tx: &Transaction) -> i64 {
        self.inner.net_value(tx)
    }
}

/// Errors which can occur when adding a keychain and descriptor to a [`KeychainTxOutIndex`].
#[derive(Clone, Debug, PartialEq)]
pub enum AddKeychainError<K: Clone + Ord + Debug> {
    /// Keychain can not be added because it was already added to the [`KeychainTxOutIndex`] and
    /// associated with a different [`Descriptor`].
    KeychainExists {
        /// Existing keychain.
        keychain: K,
        /// Descriptor currently associated to keychain.
        descriptor: Descriptor<DescriptorPublicKey>,
    },
    /// Keychain can not be added because the associated [`Descriptor`] was already added to
    /// the [`KeychainTxOutIndex`] but associated with a different keychain.
    DescriptorExists {
        /// Keychain currently associated to descriptor.
        keychain: K,
        /// Existing descriptor.
        descriptor: Descriptor<DescriptorPublicKey>,
    },
}

impl<K: Clone + Ord + Debug> core::fmt::Display for AddKeychainError<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self {
            AddKeychainError::KeychainExists {
                keychain: k,
                descriptor: d,
            } => {
                write!(
                    f,
                    "cannot add keychain `{:?}` because it was already added but is associated with a different descriptor `{}`",
                    k, d
                )
            }
            AddKeychainError::DescriptorExists {
                keychain: k,
                descriptor: d,
            } => {
                write!(
                    f,
                    "cannot add keychain because the associated descriptor `{}` was already added but is associated with a different keychain `{:?}`",
                    d, k
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl<K: Clone + Ord + Debug> std::error::Error for AddKeychainError<K> {}

impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Return the map of the keychain to descriptors.
    pub fn keychains(&self) -> impl Iterator<Item = (K, &Descriptor<DescriptorPublicKey>)> + '_ {
        self.keychains_to_descriptors
            .iter()
            .map(|(k, (_, d))| (k.clone(), d))
    }

    /// Add a keychain to the tracker's `txout_index` with a descriptor to derive addresses.
    ///
    /// Adding a keychain means you will be able to derive new script pubkeys under that keychain
    /// and the txout index will discover transaction outputs with those script pubkeys.
    ///
    /// When trying to add a keychain that already existed under a different descriptor, or a descriptor
    /// that already existed with a different keychain, the old keychain (or descriptor) will be
    /// overwritten.
    pub fn add_keychain(
        &mut self,
        keychain: K,
        descriptor: Descriptor<DescriptorPublicKey>,
    ) -> Result<ChangeSet<K>, AddKeychainError<K>> {
        let descriptor_id = calc_descriptor_id(&descriptor);

        match self.keychains_to_descriptors.get(&keychain) {
            // If the keychain exists but is associated to a different descriptor.
            Some((_d_id, d)) if d.ne(&descriptor) => {
                return Err(AddKeychainError::KeychainExists {
                    keychain,
                    descriptor: d.clone(),
                })
            }
            // If the keychain exists but is associated to the same descriptor return empty ChangeSet.
            Some(_) => {
                return Ok(ChangeSet {
                    keychains_added: Default::default(),
                    last_revealed: Default::default(),
                })
            }
            // No existing descriptor associated with this keychain.
            _ => (),
        }

        match self.descriptor_ids_to_keychain.get(&descriptor_id) {
            // If the descriptor exists but is associated to a different keychain.
            Some(k) if k.ne(&keychain) => {
                return Err(AddKeychainError::DescriptorExists {
                    keychain: k.clone(),
                    descriptor,
                })
            }
            // If the descriptor exists but is associated to the same keychain return empty ChangeSet.
            Some(_) => {
                return Ok(ChangeSet {
                    keychains_added: Default::default(),
                    last_revealed: Default::default(),
                })
            }
            // No existing keychain associated with this descriptor.
            _ => (),
        }

        self.keychains_to_descriptors
            .insert(keychain.clone(), (descriptor_id, descriptor.clone()));
        self.descriptor_ids_to_keychain
            .insert(descriptor_id, keychain.clone());
        self.replenish_lookahead(&keychain, self.lookahead);

        Ok(ChangeSet {
            keychains_added: [(keychain, descriptor)].into_iter().collect(),
            last_revealed: Default::default(),
        })
    }

    /// Get the lookahead setting.
    ///
    /// Refer to [`new`] for more information on the `lookahead`.
    ///
    /// [`new`]: Self::new
    pub fn lookahead(&self) -> u32 {
        self.lookahead
    }

    /// Store lookahead scripts until `target_index`.
    ///
    /// This does not change the `lookahead` setting.
    pub fn lookahead_to_target(&mut self, keychain: &K, target_index: u32) {
        let next_index = self.next_store_index(keychain);
        if let Some(temp_lookahead) = target_index.checked_sub(next_index).filter(|&v| v > 0) {
            self.replenish_lookahead(keychain, temp_lookahead);
        }
    }

    fn replenish_lookahead(&mut self, keychain: &K, lookahead: u32) {
        let (descriptor_id, descriptor) = self
            .descriptor_of_keychain(keychain)
            .expect("keychain must exist")
            .clone();
        let next_store_index = self.next_store_index(keychain);
        let next_reveal_index = self.last_revealed.get(&descriptor_id).map_or(0, |v| *v + 1);

        for (new_index, new_spk) in SpkIterator::new_with_range(
            descriptor.clone(),
            next_store_index..next_reveal_index + lookahead,
        ) {
            let _inserted = self.inner.insert_spk((descriptor_id, new_index), new_spk);
            debug_assert!(_inserted, "replenish lookahead: must not have existing spk: keychain={:?}, lookahead={}, next_store_index={}, next_reveal_index={}", keychain, lookahead, next_store_index, next_reveal_index);
        }
    }

    fn next_store_index(&self, keychain: &K) -> u32 {
        let descriptor_id = self
            .descriptor_of_keychain(keychain)
            .expect("keychain must exist")
            .0;
        self.inner()
            .all_spks()
            // This range is filtering out the spks with a keychain different than
            // `keychain`. We don't use filter here as range is more optimized.
            .range((descriptor_id, u32::MIN)..(descriptor_id, u32::MAX))
            .last()
            .map_or(0, |((_, index), _)| *index + 1)
    }

    /// Get an unbounded spk iterator over a given `keychain`.
    ///
    /// # Panics
    ///
    /// This will panic if the given `keychain`'s descriptor does not exist.
    pub fn unbounded_spk_iter(&self, keychain: &K) -> SpkIterator<Descriptor<DescriptorPublicKey>> {
        let descriptor = self
            .descriptor_of_keychain(keychain)
            .expect("Keychain must exist")
            .1
            .clone();
        SpkIterator::new(descriptor)
    }

    /// Get unbounded spk iterators for all keychains.
    pub fn all_unbounded_spk_iters(
        &self,
    ) -> BTreeMap<K, SpkIterator<Descriptor<DescriptorPublicKey>>> {
        self.keychains_to_descriptors
            .iter()
            .map(|(k, (_, descriptor))| (k.clone(), SpkIterator::new(descriptor.clone())))
            .collect()
    }

    /// Iterate over revealed spks of all keychains.
    pub fn revealed_spks(&self) -> impl DoubleEndedIterator<Item = (K, u32, &Script)> + Clone {
        self.keychains_to_descriptors.keys().flat_map(|keychain| {
            self.revealed_keychain_spks(keychain)
                .map(|(i, spk)| (keychain.clone(), i, spk))
        })
    }

    /// Iterate over revealed spks of the given `keychain`.
    ///
    /// # Panics
    ///
    /// This will panic if the given `keychain`'s descriptor does not exist.
    pub fn revealed_keychain_spks(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, &Script)> + Clone {
        let desc_id = self.descriptor_of_keychain(keychain).expect("Must exist").0;
        let next_i = self.last_revealed.get(&desc_id).map_or(0, |&i| i + 1);
        self.inner
            .all_spks()
            .range((desc_id, u32::MIN)..(desc_id, next_i))
            .map(|((_, i), spk)| (*i, spk.as_script()))
    }

    /// Iterate over revealed, but unused, spks of all keychains.
    pub fn unused_spks(&self) -> impl DoubleEndedIterator<Item = (K, u32, &Script)> + Clone {
        self.keychains_to_descriptors.keys().flat_map(|keychain| {
            self.unused_keychain_spks(keychain)
                .map(|(i, spk)| (keychain.clone(), i, spk))
        })
    }

    /// Iterate over revealed, but unused, spks of the given `keychain`.
    ///
    /// # Panics
    ///
    /// This will panic if the given `keychain`'s descriptor does not exist.
    pub fn unused_keychain_spks(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, &Script)> + Clone {
        let desc_id = self.descriptor_of_keychain(keychain).expect("Must exist").0;
        let next_i = self.last_revealed.get(&desc_id).map_or(0, |&i| i + 1);
        self.inner
            .unused_spks((desc_id, u32::MIN)..(desc_id, next_i))
            .map(|((_, i), spk)| (*i, spk))
    }

    /// Get the next derivation index for `keychain`. The next index is the index after the last revealed
    /// derivation index.
    ///
    /// The second field in the returned tuple represents whether the next derivation index is new.
    /// There are two scenarios where the next derivation index is reused (not new):
    ///
    /// 1. The keychain's descriptor has no wildcard, and a script has already been revealed.
    /// 2. The number of revealed scripts has already reached 2^31 (refer to BIP-32).
    ///
    /// Not checking the second field of the tuple may result in address reuse.
    ///
    /// # Panics
    ///
    /// Panics if the `keychain` does not exist.
    pub fn next_index(&self, keychain: &K) -> (u32, bool) {
        let (descriptor_id, descriptor) =
            self.descriptor_of_keychain(keychain).expect("must exist");
        let last_index = self.last_revealed.get(descriptor_id).cloned();

        // we can only get the next index if the wildcard exists.
        let has_wildcard = descriptor.has_wildcard();

        match last_index {
            // if there is no index, next_index is always 0.
            None => (0, true),
            // descriptors without wildcards can only have one index.
            Some(_) if !has_wildcard => (0, false),
            // derivation index must be < 2^31 (BIP-32).
            Some(index) if index > BIP32_MAX_INDEX => {
                unreachable!("index is out of bounds")
            }
            Some(index) if index == BIP32_MAX_INDEX => (index, false),
            // get the next derivation index.
            Some(index) => (index + 1, true),
        }
    }

    /// Get the last derivation index that is revealed for each keychain.
    ///
    /// Keychains with no revealed indices will not be included in the returned [`BTreeMap`].
    pub fn last_revealed_indices(&self) -> BTreeMap<K, u32> {
        self.last_revealed
            .iter()
            .map(|(descriptor_id, index)| {
                (
                    self.keychain_of_descriptor_id(descriptor_id)
                        .expect("Must be here")
                        .clone(),
                    *index,
                )
            })
            .collect()
    }

    /// Get the last derivation index revealed for `keychain`.
    ///
    /// # Panics
    ///
    /// Panics if the `keychain` does not exist.
    pub fn last_revealed_index(&self, keychain: &K) -> Option<u32> {
        let descriptor_id = self
            .keychains_to_descriptors
            .get(keychain)
            .expect("keychain must exist")
            .0;
        self.last_revealed.get(&descriptor_id).cloned()
    }

    /// Convenience method to call [`Self::reveal_to_target`] on multiple keychains.
    ///
    /// # Panics
    ///
    /// Panics if any keychain in `keychains` does not exist.
    pub fn reveal_to_target_multi(
        &mut self,
        keychains: &BTreeMap<K, u32>,
    ) -> (
        BTreeMap<K, SpkIterator<Descriptor<DescriptorPublicKey>>>,
        super::ChangeSet<K>,
    ) {
        let mut changeset = super::ChangeSet::default();
        let mut spks = BTreeMap::new();

        for (keychain, &index) in keychains {
            let (new_spks, new_changeset) = self.reveal_to_target(keychain, index);
            if !new_changeset.is_empty() {
                spks.insert(keychain.clone(), new_spks);
                changeset.append(new_changeset.clone());
            }
        }

        (spks, changeset)
    }

    /// Reveals script pubkeys of the `keychain`'s descriptor **up to and including** the
    /// `target_index`.
    ///
    /// If the `target_index` cannot be reached (due to the descriptor having no wildcard and/or
    /// the `target_index` is in the hardened index range), this method will make a best-effort and
    /// reveal up to the last possible index.
    ///
    /// This returns an iterator of newly revealed indices (alongside their scripts) and a
    /// [`super::ChangeSet`], which reports updates to the latest revealed index. If no new script
    /// pubkeys are revealed, then both of these will be empty.
    ///
    /// # Panics
    ///
    /// Panics if `keychain` does not exist.
    pub fn reveal_to_target(
        &mut self,
        keychain: &K,
        target_index: u32,
    ) -> (
        SpkIterator<Descriptor<DescriptorPublicKey>>,
        super::ChangeSet<K>,
    ) {
        let (descriptor_id, descriptor) =
            self.descriptor_of_keychain(keychain).expect("must exist");
        // Cloning since I need to modify self.inner, and I can't do that while
        // I'm borrowing descriptor_id and descriptor
        let (descriptor_id, descriptor) = (*descriptor_id, descriptor.clone());
        let has_wildcard = descriptor.has_wildcard();

        let target_index = if has_wildcard { target_index } else { 0 };
        let next_reveal_index = self
            .last_revealed
            .get(&descriptor_id)
            .map_or(0, |index| *index + 1);

        debug_assert!(next_reveal_index + self.lookahead >= self.next_store_index(keychain));

        // If the target_index is already revealed, we are done
        if next_reveal_index > target_index {
            return (
                SpkIterator::new_with_range(descriptor, next_reveal_index..next_reveal_index),
                super::ChangeSet::default(),
            );
        }

        // We range over the indexes that are not stored and insert their spks in the index.
        // Indexes from next_reveal_index to next_reveal_index + lookahead are already stored (due
        // to lookahead), so we only range from next_reveal_index + lookahead to target + lookahead
        let range = next_reveal_index + self.lookahead..=target_index + self.lookahead;
        for (new_index, new_spk) in SpkIterator::new_with_range(descriptor.clone(), range) {
            let _inserted = self.inner.insert_spk((descriptor_id, new_index), new_spk);
            debug_assert!(_inserted, "must not have existing spk");
            debug_assert!(
                has_wildcard || new_index == 0,
                "non-wildcard descriptors must not iterate past index 0"
            );
        }

        let _old_index = self.last_revealed.insert(descriptor_id, target_index);
        debug_assert!(_old_index < Some(target_index));
        (
            SpkIterator::new_with_range(descriptor, next_reveal_index..target_index + 1),
            super::ChangeSet {
                keychains_added: BTreeMap::new(),
                last_revealed: core::iter::once((descriptor_id, target_index)).collect(),
            },
        )
    }

    /// Attempts to reveal the next script pubkey for `keychain`.
    ///
    /// Returns the derivation index of the revealed script pubkey, the revealed script pubkey and a
    /// [`super::ChangeSet`] which represents changes in the last revealed index (if any).
    ///
    /// When a new script cannot be revealed, we return the last revealed script and an empty
    /// [`super::ChangeSet`]. There are two scenarios when a new script pubkey cannot be derived:
    ///
    ///  1. The descriptor has no wildcard and already has one script revealed.
    ///  2. The descriptor has already revealed scripts up to the numeric bound.
    ///
    /// # Panics
    ///
    /// Panics if the `keychain` does not exist.
    pub fn reveal_next_spk(&mut self, keychain: &K) -> ((u32, &Script), super::ChangeSet<K>) {
        let descriptor_id = self.descriptor_of_keychain(keychain).expect("Must exist").0;
        let (next_index, _) = self.next_index(keychain);
        let changeset = self.reveal_to_target(keychain, next_index).1;
        let script = self
            .inner
            .spk_at_index(&(descriptor_id, next_index))
            .expect("script must already be stored");
        ((next_index, script), changeset)
    }

    /// Gets the next unused script pubkey in the keychain. I.e., the script pubkey with the lowest
    /// index that has not been used yet.
    ///
    /// This will derive and reveal a new script pubkey if no more unused script pubkeys exist.
    ///
    /// If the descriptor has no wildcard and already has a used script pubkey or if a descriptor
    /// has used all scripts up to the derivation bounds, then the last derived script pubkey will be
    /// returned.
    ///
    /// # Panics
    ///
    /// Panics if `keychain` has never been added to the index
    pub fn next_unused_spk(&mut self, keychain: &K) -> ((u32, &Script), super::ChangeSet<K>) {
        let need_new = self.unused_keychain_spks(keychain).next().is_none();
        // this rather strange branch is needed because of some lifetime issues
        if need_new {
            self.reveal_next_spk(keychain)
        } else {
            (
                self.unused_keychain_spks(keychain)
                    .next()
                    .expect("we already know next exists"),
                super::ChangeSet::default(),
            )
        }
    }

    /// Iterate over all [`OutPoint`]s that point to `TxOut`s with script pubkeys derived from
    /// `keychain`.
    ///
    /// Use [`keychain_outpoints_in_range`](KeychainTxOutIndex::keychain_outpoints_in_range) to
    /// iterate over a specific derivation range.
    ///
    /// # Panics
    ///
    /// Panics if `keychain` has never been added to the index
    pub fn keychain_outpoints(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, OutPoint)> + '_ {
        self.keychain_outpoints_in_range(keychain, ..)
    }

    /// Iterate over [`OutPoint`]s that point to `TxOut`s with script pubkeys derived from
    /// `keychain` in a given derivation `range`.
    ///
    /// # Panics
    ///
    /// Panics if `keychain` has never been added to the index
    pub fn keychain_outpoints_in_range(
        &self,
        keychain: &K,
        range: impl RangeBounds<u32>,
    ) -> impl DoubleEndedIterator<Item = (u32, OutPoint)> + '_ {
        let descriptor_id = self
            .keychains_to_descriptors
            .get(keychain)
            .expect("Must exist")
            .0;
        let start = match range.start_bound() {
            Bound::Included(i) => Bound::Included((descriptor_id, *i)),
            Bound::Excluded(i) => Bound::Excluded((descriptor_id, *i)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match range.end_bound() {
            Bound::Included(i) => Bound::Included((descriptor_id, *i)),
            Bound::Excluded(i) => Bound::Excluded((descriptor_id, *i)),
            Bound::Unbounded => Bound::Unbounded,
        };
        self.inner
            .outputs_in_range((start, end))
            .map(|((_, i), op)| (*i, op))
    }

    /// Returns the highest derivation index of the `keychain` where [`KeychainTxOutIndex`] has
    /// found a [`TxOut`] with it's script pubkey.
    pub fn last_used_index(&self, keychain: &K) -> Option<u32> {
        self.keychain_outpoints(keychain).last().map(|(i, _)| i)
    }

    /// Returns the highest derivation index of each keychain that [`KeychainTxOutIndex`] has found
    /// a [`TxOut`] with it's script pubkey.
    pub fn last_used_indices(&self) -> BTreeMap<K, u32> {
        self.keychains_to_descriptors
            .iter()
            .filter_map(|(keychain, _)| {
                self.last_used_index(keychain)
                    .map(|index| (keychain.clone(), index))
            })
            .collect()
    }

    /// Applies the derivation changeset to the [`KeychainTxOutIndex`], extending the number of
    /// derived scripts per keychain, as specified in the `changeset`.
    ///
    /// A [`AddKeychainError`] error is returned if a keychain or descriptor being added via the
    /// changeset is already being used in the [`KeychainTxOutIndex`].
    pub fn apply_changeset(&mut self, changeset: super::ChangeSet<K>) {
        let ChangeSet {
            keychains_added,
            last_revealed,
        } = changeset;
        for (keychain, descriptor) in keychains_added {
            _ = self
                .add_keychain(keychain, descriptor)
                .expect("Can't reuse keychain or descriptor.");
        }
        let last_revealed = last_revealed
            .into_iter()
            .map(|(descriptor_id, index)| {
                (
                    self.keychain_of_descriptor_id(&descriptor_id)
                        .expect("Must be here")
                        .clone(),
                    index,
                )
            })
            .collect();
        let _ = self.reveal_to_target_multi(&last_revealed);
    }
}

fn calc_descriptor_id(desc: &Descriptor<DescriptorPublicKey>) -> DescriptorId {
    let mut eng = miniscript::descriptor::checksum::Engine::new();
    eng.input(&desc.to_string()).expect("must!");
    let mut id = DescriptorId::default();
    // converting from chars to u8 (try_map is still unstable!)
    for (i, c) in eng.checksum_chars().into_iter().enumerate() {
        id[i] = c as u8;
    }
    id
}
