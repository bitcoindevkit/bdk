use crate::{
    collections::*,
    indexed_tx_graph::Indexer,
    miniscript::{Descriptor, DescriptorPublicKey},
    spk_iter::BIP32_MAX_INDEX,
    DescriptorExt, DescriptorId, SpkIterator, SpkTxOutIndex,
};
use alloc::borrow::ToOwned;
use bitcoin::{
    hashes::Hash, Amount, OutPoint, Script, ScriptBuf, SignedAmount, Transaction, TxOut, Txid,
};
use core::{
    fmt::Debug,
    ops::{Bound, RangeBounds},
};

use crate::Append;

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
    fn append(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        self.keychains_added.extend(other.keychains_added);

        // for `last_revealed`, entries of `other` will take precedence ONLY if it is greater than
        // what was originally in `self`.
        for (desc_id, index) in other.last_revealed {
            use crate::collections::btree_map::Entry;
            match self.last_revealed.entry(desc_id) {
                Entry::Vacant(entry) => {
                    entry.insert(index);
                }
                Entry::Occupied(mut entry) => {
                    if *entry.get() < index {
                        entry.insert(index);
                    }
                }
            }
        }
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

const DEFAULT_LOOKAHEAD: u32 = 25;

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
/// default `lookahead` count is 1000. Use [`new`] to set a custom `lookahead`.
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
/// Methods that can update the last revealed index or add keychains will return [`ChangeSet`] to
/// report these changes. This can be persisted for future recovery.
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
/// # let (descriptor_42, _) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/2/*)").unwrap();
/// let _ = txout_index.insert_descriptor(MyKeychain::External, external_descriptor);
/// let _ = txout_index.insert_descriptor(MyKeychain::Internal, internal_descriptor);
/// let _ = txout_index.insert_descriptor(MyKeychain::MyAppUser { user_id: 42 }, descriptor_42);
///
/// let new_spk_for_user = txout_index.reveal_next_spk(&MyKeychain::MyAppUser{ user_id: 42 });
/// ```
///
/// # Non-recommend keychain to descriptor assignments
///
/// A keychain (`K`) is used to identify a descriptor. However, the following keychain to descriptor
/// arrangements result in behavior that is harder to reason about and is not recommended.
///
/// ## Multiple keychains identifying the same descriptor
///
/// Although a single keychain variant can only identify a single descriptor, multiple keychain
/// variants can identify the same descriptor.
///
/// If multiple keychains identify the same descriptor:
/// 1. Methods that take in a keychain (such as [`reveal_next_spk`]) will work normally when any
/// keychain (that identifies that descriptor) is passed in.
/// 2. Methods that return data which associates with a descriptor (such as [`outpoints`],
/// [`txouts`], [`unused_spks`], etc.) the method will return the highest-ranked keychain variant
/// that identifies the descriptor. Rank is determined by the [`Ord`] implementation of the keychain
/// type.
///
/// This arrangement is not recommended since some methods will return a single keychain variant
/// even though multiple keychain variants identify the same descriptor.
///
/// ## Reassigning the descriptor of a single keychain
///
/// Descriptors added to [`KeychainTxOutIndex`] are never removed. However, a keychain that
/// identifies a descriptor can be reassigned to identify a different descriptor. This may result in
/// a situation where a descriptor has no associated keychain(s), and relevant [`TxOut`]s,
/// [`OutPoint`]s and [`Script`]s (of that descriptor) will not be return by [`KeychainTxOutIndex`].
/// Therefore, reassigning the descriptor of a single keychain is not recommended.
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
/// [`outpoints`]: KeychainTxOutIndex::outpoints
/// [`txouts`]: KeychainTxOutIndex::txouts
/// [`unused_spks`]: KeychainTxOutIndex::unused_spks
#[derive(Clone, Debug)]
pub struct KeychainTxOutIndex<K> {
    inner: SpkTxOutIndex<(DescriptorId, u32)>,
    // keychain -> descriptor_id map
    keychains_to_descriptor_ids: BTreeMap<K, DescriptorId>,
    // descriptor_id -> keychain set
    // This is a reverse map of `keychains_to_descriptors`. Although there is only one descriptor
    // per keychain, different keychains can refer to the same descriptor, therefore we have a set
    // of keychains per descriptor. When associated data (such as spks, outpoints) are returned with
    // a keychain, we return it with the highest-ranked keychain with it. We rank keychains by
    // `Ord`, therefore the keychain set is a `BTreeSet`. The earliest keychain variant (according
    // to `Ord`) has precedence.
    keychains: HashMap<DescriptorId, BTreeSet<K>>,
    // descriptor_id -> descriptor map
    // This is a "monotone" map, meaning that its size keeps growing, i.e., we never delete
    // descriptors from it. This is useful for revealing spks for descriptors that don't have
    // keychains associated.
    descriptors: BTreeMap<DescriptorId, Descriptor<DescriptorPublicKey>>,
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
    type ChangeSet = ChangeSet<K>;

    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::ChangeSet {
        match self.inner.scan_txout(outpoint, txout).cloned() {
            Some((descriptor_id, index)) => {
                // We want to reveal spks for descriptors that aren't tracked by any keychain, and
                // so we call reveal with descriptor_id
                let desc = self
                    .descriptors
                    .get(&descriptor_id)
                    .cloned()
                    .expect("descriptors are added monotonically, scanned txout descriptor ids must have associated descriptors");
                let (_, changeset) = self.reveal_to_target_with_descriptor(desc, index);
                changeset
            }
            None => ChangeSet::default(),
        }
    }

    fn index_tx(&mut self, tx: &bitcoin::Transaction) -> Self::ChangeSet {
        let mut changeset = ChangeSet::<K>::default();
        for (op, txout) in tx.output.iter().enumerate() {
            changeset.append(self.index_txout(OutPoint::new(tx.txid(), op as u32), txout));
        }
        changeset
    }

    fn initial_changeset(&self) -> Self::ChangeSet {
        ChangeSet {
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
            keychains_to_descriptor_ids: BTreeMap::new(),
            keychains: HashMap::new(),
            descriptors: BTreeMap::new(),
            last_revealed: BTreeMap::new(),
            lookahead,
        }
    }
}

/// Methods that are *re-exposed* from the internal [`SpkTxOutIndex`].
impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Get the highest-ranked keychain that is currently associated with the given `desc_id`.
    fn keychain_of_desc_id(&self, desc_id: &DescriptorId) -> Option<&K> {
        let keychains = self.keychains.get(desc_id)?;
        keychains.iter().next()
    }

    /// Return a reference to the internal [`SpkTxOutIndex`].
    ///
    /// **WARNING:** The internal index will contain lookahead spks. Refer to
    /// [struct-level docs](KeychainTxOutIndex) for more about `lookahead`.
    pub fn inner(&self) -> &SpkTxOutIndex<(DescriptorId, u32)> {
        &self.inner
    }

    /// Get the set of indexed outpoints, corresponding to tracked keychains.
    pub fn outpoints(&self) -> impl DoubleEndedIterator<Item = ((K, u32), OutPoint)> + '_ {
        self.inner
            .outpoints()
            .iter()
            .filter_map(|((desc_id, index), op)| {
                let keychain = self.keychain_of_desc_id(desc_id)?;
                Some(((keychain.clone(), *index), *op))
            })
    }

    /// Iterate over known txouts that spend to tracked script pubkeys.
    pub fn txouts(&self) -> impl DoubleEndedIterator<Item = (K, u32, OutPoint, &TxOut)> + '_ {
        self.inner.txouts().filter_map(|((desc_id, i), op, txo)| {
            let keychain = self.keychain_of_desc_id(desc_id)?;
            Some((keychain.clone(), *i, op, txo))
        })
    }

    /// Finds all txouts on a transaction that has previously been scanned and indexed.
    pub fn txouts_in_tx(
        &self,
        txid: Txid,
    ) -> impl DoubleEndedIterator<Item = (K, u32, OutPoint, &TxOut)> {
        self.inner
            .txouts_in_tx(txid)
            .filter_map(|((desc_id, i), op, txo)| {
                let keychain = self.keychain_of_desc_id(desc_id)?;
                Some((keychain.clone(), *i, op, txo))
            })
    }

    /// Return the [`TxOut`] of `outpoint` if it has been indexed, and if it corresponds to a
    /// tracked keychain.
    ///
    /// The associated keychain and keychain index of the txout's spk is also returned.
    ///
    /// This calls [`SpkTxOutIndex::txout`] internally.
    pub fn txout(&self, outpoint: OutPoint) -> Option<(K, u32, &TxOut)> {
        let ((descriptor_id, index), txo) = self.inner.txout(outpoint)?;
        let keychain = self.keychain_of_desc_id(descriptor_id)?;
        Some((keychain.clone(), *index, txo))
    }

    /// Return the script that exists under the given `keychain`'s `index`.
    ///
    /// This calls [`SpkTxOutIndex::spk_at_index`] internally.
    pub fn spk_at_index(&self, keychain: K, index: u32) -> Option<&Script> {
        let descriptor_id = *self.keychains_to_descriptor_ids.get(&keychain)?;
        self.inner.spk_at_index(&(descriptor_id, index))
    }

    /// Returns the keychain and keychain index associated with the spk.
    ///
    /// This calls [`SpkTxOutIndex::index_of_spk`] internally.
    pub fn index_of_spk(&self, script: &Script) -> Option<(K, u32)> {
        let (desc_id, last_index) = self.inner.index_of_spk(script)?;
        let keychain = self.keychain_of_desc_id(desc_id)?;
        Some((keychain.clone(), *last_index))
    }

    /// Returns whether the spk under the `keychain`'s `index` has been used.
    ///
    /// Here, "unused" means that after the script pubkey was stored in the index, the index has
    /// never scanned a transaction output with it.
    ///
    /// This calls [`SpkTxOutIndex::is_used`] internally.
    pub fn is_used(&self, keychain: K, index: u32) -> bool {
        let descriptor_id = self.keychains_to_descriptor_ids.get(&keychain).copied();
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
    /// Returns whether the spk under the given `keychain` and `index` is successfully
    /// marked as used. Returns false either when there is no descriptor under the given
    /// keychain, or when the spk is already marked as used.
    ///
    /// This is useful when you want to reserve a script pubkey for something but don't want to add
    /// the transaction output using it to the index yet. Other callers will consider `index` on
    /// `keychain` used until you call [`unmark_used`].
    ///
    /// This calls [`SpkTxOutIndex::mark_used`] internally.
    ///
    /// [`unmark_used`]: Self::unmark_used
    pub fn mark_used(&mut self, keychain: K, index: u32) -> bool {
        let descriptor_id = self.keychains_to_descriptor_ids.get(&keychain).copied();
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
        let descriptor_id = self.keychains_to_descriptor_ids.get(&keychain).copied();
        match descriptor_id {
            Some(descriptor_id) => self.inner.unmark_used(&(descriptor_id, index)),
            None => false,
        }
    }

    /// Computes the total value transfer effect `tx` has on the script pubkeys belonging to the
    /// keychains in `range`. Value is *sent* when a script pubkey in the `range` is on an input and
    /// *received* when it is on an output. For `sent` to be computed correctly, the output being
    /// spent must have already been scanned by the index. Calculating received just uses the
    /// [`Transaction`] outputs directly, so it will be correct even if it has not been scanned.
    pub fn sent_and_received(
        &self,
        tx: &Transaction,
        range: impl RangeBounds<K>,
    ) -> (Amount, Amount) {
        self.inner
            .sent_and_received(tx, self.map_to_inner_bounds(range))
    }

    /// Computes the net value that this transaction gives to the script pubkeys in the index and
    /// *takes* from the transaction outputs in the index. Shorthand for calling
    /// [`sent_and_received`] and subtracting sent from received.
    ///
    /// This calls [`SpkTxOutIndex::net_value`] internally.
    ///
    /// [`sent_and_received`]: Self::sent_and_received
    pub fn net_value(&self, tx: &Transaction, range: impl RangeBounds<K>) -> SignedAmount {
        self.inner.net_value(tx, self.map_to_inner_bounds(range))
    }
}

impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Return the map of the keychain to descriptors.
    pub fn keychains(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&K, &Descriptor<DescriptorPublicKey>)> + ExactSizeIterator + '_
    {
        self.keychains_to_descriptor_ids.iter().map(|(k, desc_id)| {
            let descriptor = self
                .descriptors
                .get(desc_id)
                .expect("descriptor id cannot be associated with keychain without descriptor");
            (k, descriptor)
        })
    }

    /// Insert a descriptor with a keychain associated to it.
    ///
    /// Adding a descriptor means you will be able to derive new script pubkeys under it
    /// and the txout index will discover transaction outputs with those script pubkeys.
    ///
    /// When trying to add a keychain that already existed under a different descriptor, or a descriptor
    /// that already existed with a different keychain, the old keychain (or descriptor) will be
    /// overwritten.
    pub fn insert_descriptor(
        &mut self,
        keychain: K,
        descriptor: Descriptor<DescriptorPublicKey>,
    ) -> ChangeSet<K> {
        let mut changeset = ChangeSet::<K>::default();
        let desc_id = descriptor.descriptor_id();

        let old_desc_id = self
            .keychains_to_descriptor_ids
            .insert(keychain.clone(), desc_id);

        if let Some(old_desc_id) = old_desc_id {
            // nothing needs to be done if caller reinsterted the same descriptor under the same
            // keychain
            if old_desc_id == desc_id {
                return changeset;
            }
            // remove keychain from reverse index
            let _is_keychain_removed = self
                .keychains
                .get_mut(&old_desc_id)
                .expect("we must have already inserted this descriptor")
                .remove(&keychain);
            debug_assert!(_is_keychain_removed);
        }

        self.keychains
            .entry(desc_id)
            .or_default()
            .insert(keychain.clone());
        self.descriptors.insert(desc_id, descriptor.clone());
        self.replenish_lookahead(&keychain, self.lookahead);

        changeset
            .keychains_added
            .insert(keychain.clone(), descriptor);
        changeset
    }

    /// Gets the descriptor associated with the keychain. Returns `None` if the keychain doesn't
    /// have a descriptor associated with it.
    pub fn get_descriptor(&self, keychain: &K) -> Option<&Descriptor<DescriptorPublicKey>> {
        self.keychains_to_descriptor_ids
            .get(keychain)
            .map(|desc_id| {
                self.descriptors
                    .get(desc_id)
                    .expect("descriptor id cannot be associated with keychain without descriptor")
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

    /// Store lookahead scripts until `target_index` (inclusive).
    ///
    /// This does not change the global `lookahead` setting.
    pub fn lookahead_to_target(&mut self, keychain: &K, target_index: u32) {
        if let Some((next_index, _)) = self.next_index(keychain) {
            let temp_lookahead = (target_index + 1)
                .checked_sub(next_index)
                .filter(|&index| index > 0);

            if let Some(temp_lookahead) = temp_lookahead {
                self.replenish_lookahead(keychain, temp_lookahead);
            }
        }
    }

    fn replenish_lookahead(&mut self, keychain: &K, lookahead: u32) {
        let descriptor_id = self.keychains_to_descriptor_ids.get(keychain).copied();
        if let Some(descriptor_id) = descriptor_id {
            let descriptor = self
                .descriptors
                .get(&descriptor_id)
                .expect("descriptor id cannot be associated with keychain without descriptor");

            let next_store_index = self.next_store_index(descriptor_id);
            let next_reveal_index = self.last_revealed.get(&descriptor_id).map_or(0, |v| *v + 1);

            for (new_index, new_spk) in SpkIterator::new_with_range(
                descriptor,
                next_store_index..next_reveal_index + lookahead,
            ) {
                let _inserted = self.inner.insert_spk((descriptor_id, new_index), new_spk);
                debug_assert!(_inserted, "replenish lookahead: must not have existing spk: keychain={:?}, lookahead={}, next_store_index={}, next_reveal_index={}", keychain, lookahead, next_store_index, next_reveal_index);
            }
        }
    }

    fn next_store_index(&self, descriptor_id: DescriptorId) -> u32 {
        self.inner()
            .all_spks()
            // This range is keeping only the spks with descriptor_id equal to
            // `descriptor_id`. We don't use filter here as range is more optimized.
            .range((descriptor_id, u32::MIN)..(descriptor_id, u32::MAX))
            .last()
            .map_or(0, |((_, index), _)| *index + 1)
    }

    /// Get an unbounded spk iterator over a given `keychain`. Returns `None` if the provided
    /// keychain doesn't exist
    pub fn unbounded_spk_iter(
        &self,
        keychain: &K,
    ) -> Option<SpkIterator<Descriptor<DescriptorPublicKey>>> {
        let desc_id = self.keychains_to_descriptor_ids.get(keychain)?;
        let desc = self
            .descriptors
            .get(desc_id)
            .cloned()
            .expect("descriptor id cannot be associated with keychain without descriptor");
        Some(SpkIterator::new(desc))
    }

    /// Get unbounded spk iterators for all keychains.
    pub fn all_unbounded_spk_iters(
        &self,
    ) -> BTreeMap<K, SpkIterator<Descriptor<DescriptorPublicKey>>> {
        self.keychains_to_descriptor_ids
            .iter()
            .map(|(k, desc_id)| {
                let desc =
                    self.descriptors.get(desc_id).cloned().expect(
                        "descriptor id cannot be associated with keychain without descriptor",
                    );
                (k.clone(), SpkIterator::new(desc))
            })
            .collect()
    }

    /// Iterate over revealed spks of keychains in `range`
    pub fn revealed_spks(
        &self,
        range: impl RangeBounds<K>,
    ) -> impl DoubleEndedIterator<Item = (&K, u32, &Script)> + Clone {
        self.keychains_to_descriptor_ids
            .range(range)
            .flat_map(|(_, descriptor_id)| {
                let start = Bound::Included((*descriptor_id, u32::MIN));
                let end = match self.last_revealed.get(descriptor_id) {
                    Some(last_revealed) => Bound::Included((*descriptor_id, *last_revealed)),
                    None => Bound::Excluded((*descriptor_id, u32::MIN)),
                };

                self.inner
                    .all_spks()
                    .range((start, end))
                    .map(|((descriptor_id, i), spk)| {
                        (
                            self.keychain_of_desc_id(descriptor_id)
                                .expect("must have keychain"),
                            *i,
                            spk.as_script(),
                        )
                    })
            })
    }

    /// Iterate over revealed spks of the given `keychain`.
    pub fn revealed_keychain_spks<'a>(
        &'a self,
        keychain: &'a K,
    ) -> impl DoubleEndedIterator<Item = (u32, &Script)> + 'a {
        self.revealed_spks(keychain..=keychain)
            .map(|(_, i, spk)| (i, spk))
    }

    /// Iterate over revealed, but unused, spks of all keychains.
    pub fn unused_spks(&self) -> impl DoubleEndedIterator<Item = (K, u32, &Script)> + Clone {
        self.keychains_to_descriptor_ids
            .keys()
            .flat_map(|keychain| {
                self.unused_keychain_spks(keychain)
                    .map(|(i, spk)| (keychain.clone(), i, spk))
            })
    }

    /// Iterate over revealed, but unused, spks of the given `keychain`.
    /// Returns an empty iterator if the provided keychain doesn't exist.
    pub fn unused_keychain_spks(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, &Script)> + Clone {
        let desc_id = self
            .keychains_to_descriptor_ids
            .get(keychain)
            .cloned()
            // We use a dummy desc id if we can't find the real one in our map. In this way,
            // if this method was to be called with a non-existent keychain, we would return an
            // empty iterator
            .unwrap_or_else(|| DescriptorId::from_byte_array([0; 32]));
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
    /// Returns None if the provided `keychain` doesn't exist.
    pub fn next_index(&self, keychain: &K) -> Option<(u32, bool)> {
        let descriptor_id = self.keychains_to_descriptor_ids.get(keychain)?;
        let descriptor = self
            .descriptors
            .get(descriptor_id)
            .expect("descriptor id cannot be associated with keychain without descriptor");
        Some(self.next_index_with_descriptor(descriptor))
    }

    /// Get the next derivation index of given `descriptor`. The next derivation index is the index
    /// after the last revealed index.
    ///
    /// The second field of the returned tuple represents whether the next derivation index is new.
    /// There are two scenarios where the next derivation index is reused (not new):
    ///
    /// 1. The keychain's descriptor has no wildcard, and a script has already been revealed.
    /// 2. The number of revealed scripts has already reached 2^31 (refer to BIP-32).
    ///
    /// Not checking the second field of the tuple may result in address reuse.
    fn next_index_with_descriptor(
        &self,
        descriptor: &Descriptor<DescriptorPublicKey>,
    ) -> (u32, bool) {
        let desc_id = descriptor.descriptor_id();

        // we can only get the next index if the wildcard exists
        let has_wildcard = descriptor.has_wildcard();
        let last_index = self.last_revealed.get(&desc_id).cloned();

        match last_index {
            // if there is no index, next_index is always 0
            None => (0, true),
            // descriptors without wildcards can only have one index
            Some(_) if !has_wildcard => (0, false),
            // derivation index must be < 2^31 (BIP-32)
            Some(index) if index > BIP32_MAX_INDEX => unreachable!("index out of bounds"),
            Some(index) if index == BIP32_MAX_INDEX => (index, false),
            // get the next derivation index
            Some(index) => (index + 1, true),
        }
    }

    /// Get the last derivation index that is revealed for each keychain.
    ///
    /// Keychains with no revealed indices will not be included in the returned [`BTreeMap`].
    pub fn last_revealed_indices(&self) -> BTreeMap<K, u32> {
        self.last_revealed
            .iter()
            .filter_map(|(desc_id, index)| {
                let keychain = self.keychain_of_desc_id(desc_id)?;
                Some((keychain.clone(), *index))
            })
            .collect()
    }

    /// Get the last derivation index revealed for `keychain`. Returns None if the keychain doesn't
    /// exist, or if the keychain doesn't have any revealed scripts.
    pub fn last_revealed_index(&self, keychain: &K) -> Option<u32> {
        let descriptor_id = self.keychains_to_descriptor_ids.get(keychain)?;
        self.last_revealed.get(descriptor_id).cloned()
    }

    /// Convenience method to call [`Self::reveal_to_target`] on multiple keychains.
    pub fn reveal_to_target_multi(
        &mut self,
        keychains: &BTreeMap<K, u32>,
    ) -> (
        BTreeMap<K, SpkIterator<Descriptor<DescriptorPublicKey>>>,
        ChangeSet<K>,
    ) {
        let mut changeset = ChangeSet::default();
        let mut spks = BTreeMap::new();

        for (keychain, &index) in keychains {
            let (new_spks, new_changeset) = self.reveal_to_target(keychain, index);
            if let Some(new_spks) = new_spks {
                changeset.append(new_changeset);
                spks.insert(keychain.clone(), new_spks);
            }
        }

        (spks, changeset)
    }

    /// Convenience method to call `reveal_to_target` with a descriptor_id instead of a keychain.
    /// This is useful for revealing spks of descriptors for which we don't have a keychain
    /// tracked.
    /// Refer to the `reveal_to_target` documentation for more.
    ///
    /// Returns None if the provided `descriptor_id` doesn't correspond to a tracked descriptor.
    fn reveal_to_target_with_descriptor(
        &mut self,
        descriptor: Descriptor<DescriptorPublicKey>,
        target_index: u32,
    ) -> (SpkIterator<Descriptor<DescriptorPublicKey>>, ChangeSet<K>) {
        let descriptor_id = descriptor.descriptor_id();
        let has_wildcard = descriptor.has_wildcard();

        let target_index = if has_wildcard { target_index } else { 0 };
        let next_reveal_index = self
            .last_revealed
            .get(&descriptor_id)
            .map_or(0, |index| *index + 1);

        debug_assert!(next_reveal_index + self.lookahead >= self.next_store_index(descriptor_id));

        // If the target_index is already revealed, we are done
        if next_reveal_index > target_index {
            return (
                SpkIterator::new_with_range(descriptor, next_reveal_index..next_reveal_index),
                ChangeSet::default(),
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
            ChangeSet {
                keychains_added: BTreeMap::new(),
                last_revealed: core::iter::once((descriptor_id, target_index)).collect(),
            },
        )
    }

    /// Reveals script pubkeys of the `keychain`'s descriptor **up to and including** the
    /// `target_index`.
    ///
    /// If the `target_index` cannot be reached (due to the descriptor having no wildcard and/or
    /// the `target_index` is in the hardened index range), this method will make a best-effort and
    /// reveal up to the last possible index.
    ///
    /// This returns an iterator of newly revealed indices (alongside their scripts) and a
    /// [`ChangeSet`], which reports updates to the latest revealed index. If no new script pubkeys
    /// are revealed, then both of these will be empty.
    ///
    /// Returns None if the provided `keychain` doesn't exist.
    pub fn reveal_to_target(
        &mut self,
        keychain: &K,
        target_index: u32,
    ) -> (
        Option<SpkIterator<Descriptor<DescriptorPublicKey>>>,
        ChangeSet<K>,
    ) {
        let descriptor_id = match self.keychains_to_descriptor_ids.get(keychain) {
            Some(desc_id) => desc_id,
            None => return (None, ChangeSet::default()),
        };
        let desc = self
            .descriptors
            .get(descriptor_id)
            .cloned()
            .expect("descriptors are added monotonically, scanned txout descriptor ids must have associated descriptors");
        let (spk_iter, changeset) = self.reveal_to_target_with_descriptor(desc, target_index);
        (Some(spk_iter), changeset)
    }

    /// Attempts to reveal the next script pubkey for `keychain`.
    ///
    /// Returns the next revealed script pubkey (alongside it's derivation index), and a
    /// [`ChangeSet`]. The next revealed script pubkey is `None` if the provided `keychain` has no
    /// associated descriptor.
    ///
    /// When the descriptor has no more script pubkeys to reveal, the last revealed script and an
    /// empty [`ChangeSet`] is returned. There are 3 scenarios in which a descriptor can no longer
    /// derive scripts:
    ///
    ///  1. The descriptor has no wildcard and already has one script revealed.
    ///  2. The descriptor has already revealed scripts up to the numeric bound.
    ///  3. There is no descriptor associated with the given keychain.
    pub fn reveal_next_spk(&mut self, keychain: &K) -> (Option<(u32, ScriptBuf)>, ChangeSet<K>) {
        let descriptor_id = match self.keychains_to_descriptor_ids.get(keychain) {
            Some(&desc_id) => desc_id,
            None => {
                return (None, Default::default());
            }
        };
        let descriptor = self
            .descriptors
            .get(&descriptor_id)
            .expect("descriptor id cannot be associated with keychain without descriptor");
        let (next_index, _) = self.next_index_with_descriptor(descriptor);
        let (mut new_spks, changeset) =
            self.reveal_to_target_with_descriptor(descriptor.clone(), next_index);
        let revealed_spk = new_spks.next().unwrap_or_else(|| {
            // if we have exhausted all derivation indicies, the returned `next_index` is reused
            // and will not be included in `new_spks` (refer to `next_index_with_descriptor` docs)
            let spk = self
                .inner
                .spk_at_index(&(descriptor_id, next_index))
                .expect("script must already be stored")
                .to_owned();
            (next_index, spk)
        });
        debug_assert_eq!(new_spks.next(), None, "must only reveal one spk");
        (Some(revealed_spk), changeset)
    }

    /// Gets the next unused script pubkey in the keychain. I.e. the script pubkey with the lowest
    /// derivation index that has not been used (no associated transaction outputs).
    ///
    /// Returns the unused script pubkey (alongside it's derivation index), and a [`ChangeSet`].
    /// The returned script pubkey will be `None` if the provided `keychain` has no associated
    /// descriptor.
    ///
    /// This will derive and reveal a new script pubkey if no more unused script pubkeys exists
    /// (refer to [`reveal_next_spk`](Self::reveal_next_spk)).
    ///
    /// If the descriptor has no wildcard and already has a used script pubkey or if a descriptor
    /// has used all scripts up to the derivation bounds, then the last derived script pubkey will
    /// be returned.
    pub fn next_unused_spk(&mut self, keychain: &K) -> (Option<(u32, ScriptBuf)>, ChangeSet<K>) {
        let next_unused_spk = self
            .unused_keychain_spks(keychain)
            .next()
            .map(|(i, spk)| (i, spk.to_owned()));
        if next_unused_spk.is_some() {
            (next_unused_spk, Default::default())
        } else {
            self.reveal_next_spk(keychain)
        }
    }

    /// Iterate over all [`OutPoint`]s that have `TxOut`s with script pubkeys derived from
    /// `keychain`.
    pub fn keychain_outpoints<'a>(
        &'a self,
        keychain: &'a K,
    ) -> impl DoubleEndedIterator<Item = (u32, OutPoint)> + 'a {
        self.keychain_outpoints_in_range(keychain..=keychain)
            .map(move |(_, i, op)| (i, op))
    }

    /// Iterate over [`OutPoint`]s that have script pubkeys derived from keychains in `range`.
    pub fn keychain_outpoints_in_range<'a>(
        &'a self,
        range: impl RangeBounds<K> + 'a,
    ) -> impl DoubleEndedIterator<Item = (&'a K, u32, OutPoint)> + 'a {
        let bounds = self.map_to_inner_bounds(range);
        self.inner
            .outputs_in_range(bounds)
            .map(move |((desc_id, i), op)| {
                let keychain = self
                    .keychain_of_desc_id(desc_id)
                    .expect("keychain must exist");
                (keychain, *i, op)
            })
    }

    fn map_to_inner_bounds(
        &self,
        bound: impl RangeBounds<K>,
    ) -> impl RangeBounds<(DescriptorId, u32)> {
        let get_desc_id = |keychain| {
            self.keychains_to_descriptor_ids
                .get(keychain)
                .copied()
                .unwrap_or_else(|| DescriptorId::from_byte_array([0; 32]))
        };
        let start = match bound.start_bound() {
            Bound::Included(keychain) => Bound::Included((get_desc_id(keychain), u32::MIN)),
            Bound::Excluded(keychain) => Bound::Excluded((get_desc_id(keychain), u32::MAX)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match bound.end_bound() {
            Bound::Included(keychain) => Bound::Included((get_desc_id(keychain), u32::MAX)),
            Bound::Excluded(keychain) => Bound::Excluded((get_desc_id(keychain), u32::MIN)),
            Bound::Unbounded => Bound::Unbounded,
        };

        (start, end)
    }

    /// Returns the highest derivation index of the `keychain` where [`KeychainTxOutIndex`] has
    /// found a [`TxOut`] with it's script pubkey.
    pub fn last_used_index(&self, keychain: &K) -> Option<u32> {
        self.keychain_outpoints(keychain).last().map(|(i, _)| i)
    }

    /// Returns the highest derivation index of each keychain that [`KeychainTxOutIndex`] has found
    /// a [`TxOut`] with it's script pubkey.
    pub fn last_used_indices(&self) -> BTreeMap<K, u32> {
        self.keychains_to_descriptor_ids
            .iter()
            .filter_map(|(keychain, _)| {
                self.last_used_index(keychain)
                    .map(|index| (keychain.clone(), index))
            })
            .collect()
    }

    /// Applies the derivation changeset to the [`KeychainTxOutIndex`], as specified in the
    /// [`ChangeSet::append`] documentation:
    /// - Extends the number of derived scripts per keychain
    /// - Adds new descriptors introduced
    /// - If a descriptor is introduced for a keychain that already had a descriptor, overwrites
    /// the old descriptor
    pub fn apply_changeset(&mut self, changeset: ChangeSet<K>) {
        let ChangeSet {
            keychains_added,
            last_revealed,
        } = changeset;
        for (keychain, descriptor) in keychains_added {
            let _ = self.insert_descriptor(keychain, descriptor);
        }
        let last_revealed = last_revealed
            .into_iter()
            .filter_map(|(desc_id, index)| {
                let keychain = self.keychain_of_desc_id(&desc_id)?;
                Some((keychain.clone(), index))
            })
            .collect();
        let _ = self.reveal_to_target_multi(&last_revealed);
    }
}
