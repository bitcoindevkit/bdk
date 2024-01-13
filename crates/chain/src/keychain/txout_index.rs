use crate::{
    collections::*,
    indexed_tx_graph::Indexer,
    miniscript::{Descriptor, DescriptorPublicKey},
    spk_iter::BIP32_MAX_INDEX,
    SpkIterator, SpkTxOutIndex,
};
use bitcoin::{OutPoint, Script, Transaction, TxOut};
use core::fmt::Debug;

use crate::Append;

const DEFAULT_LOOKAHEAD: u32 = 1_000;

/// A convenient wrapper around [`SpkTxOutIndex`] that relates script pubkeys to miniscript public
/// [`Descriptor`]s.
///
/// Descriptors are referenced by the provided keychain generic (`K`).
///
/// Script pubkeys for a descriptor are revealed chronologically from index 0. I.e., If the last
/// revealed index of a descriptor is 5; scripts of indices 0 to 4 are guaranteed to be already
/// revealed. In addition to revealed scripts, we have a `lookahead` parameter for each keychain,
/// which defines the number of script pubkeys to store ahead of the last revealed index.
///
/// Methods that could update the last revealed index will return [`super::ChangeSet`] to report
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
#[derive(Clone, Debug)]
pub struct KeychainTxOutIndex<K> {
    inner: SpkTxOutIndex<(K, u32)>,
    // descriptors of each keychain
    keychains: BTreeMap<K, Descriptor<DescriptorPublicKey>>,
    // last revealed indexes
    last_revealed: BTreeMap<K, u32>,
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
            Some((keychain, index)) => self.reveal_to_target(&keychain, index).1,
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
        super::ChangeSet(self.last_revealed.clone())
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
    pub fn new(lookahead: u32) -> Self {
        Self {
            inner: SpkTxOutIndex::default(),
            keychains: BTreeMap::new(),
            last_revealed: BTreeMap::new(),
            lookahead,
        }
    }
}

/// Methods that are *re-exposed* from the internal [`SpkTxOutIndex`].
impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Return a reference to the internal [`SpkTxOutIndex`].
    ///
    /// **WARNING:** The internal index will contain lookahead spks.
    pub fn inner(&self) -> &SpkTxOutIndex<(K, u32)> {
        &self.inner
    }

    /// Get a reference to the set of indexed outpoints.
    pub fn outpoints(&self) -> &BTreeSet<((K, u32), OutPoint)> {
        self.inner.outpoints()
    }

    /// Return the [`TxOut`] of `outpoint` if it has been indexed.
    ///
    /// The associated keychain and keychain index of the txout's spk is also returned.
    ///
    /// This calls [`SpkTxOutIndex::txout`] internally.
    pub fn txout(&self, outpoint: OutPoint) -> Option<(K, u32, &TxOut)> {
        self.inner
            .txout(outpoint)
            .map(|((k, i), txo)| (k.clone(), *i, txo))
    }

    /// Return the script that exists under the given `keychain`'s `index`.
    ///
    /// This calls [`SpkTxOutIndex::spk_at_index`] internally.
    pub fn spk_at_index(&self, keychain: K, index: u32) -> Option<&Script> {
        self.inner.spk_at_index(&(keychain, index))
    }

    /// Returns the keychain and keychain index associated with the spk.
    ///
    /// This calls [`SpkTxOutIndex::index_of_spk`] internally.
    pub fn index_of_spk(&self, script: &Script) -> Option<(K, u32)> {
        self.inner.index_of_spk(script).cloned()
    }

    /// Returns whether the spk under the `keychain`'s `index` has been used.
    ///
    /// Here, "unused" means that after the script pubkey was stored in the index, the index has
    /// never scanned a transaction output with it.
    ///
    /// This calls [`SpkTxOutIndex::is_used`] internally.
    pub fn is_used(&self, keychain: K, index: u32) -> bool {
        self.inner.is_used(&(keychain, index))
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
        self.inner.mark_used(&(keychain, index))
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
        self.inner.unmark_used(&(keychain, index))
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

impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Return a reference to the internal map of keychain to descriptors.
    pub fn keychains(&self) -> &BTreeMap<K, Descriptor<DescriptorPublicKey>> {
        &self.keychains
    }

    /// Add a keychain to the tracker's `txout_index` with a descriptor to derive addresses.
    ///
    /// Adding a keychain means you will be able to derive new script pubkeys under that keychain
    /// and the txout index will discover transaction outputs with those script pubkeys.
    ///
    /// # Panics
    ///
    /// This will panic if a different `descriptor` is introduced to the same `keychain`.
    pub fn add_keychain(&mut self, keychain: K, descriptor: Descriptor<DescriptorPublicKey>) {
        let old_descriptor = &*self
            .keychains
            .entry(keychain.clone())
            .or_insert_with(|| descriptor.clone());
        assert_eq!(
            &descriptor, old_descriptor,
            "keychain already contains a different descriptor"
        );
        self.replenish_lookahead(&keychain, self.lookahead);
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
        let descriptor = self.keychains.get(keychain).expect("keychain must exist");
        let next_store_index = self.next_store_index(keychain);
        let next_reveal_index = self.last_revealed.get(keychain).map_or(0, |v| *v + 1);

        for (new_index, new_spk) in
            SpkIterator::new_with_range(descriptor, next_store_index..next_reveal_index + lookahead)
        {
            let _inserted = self
                .inner
                .insert_spk((keychain.clone(), new_index), new_spk);
            debug_assert!(_inserted, "replenish lookahead: must not have existing spk: keychain={:?}, lookahead={}, next_store_index={}, next_reveal_index={}", keychain, lookahead, next_store_index, next_reveal_index);
        }
    }

    fn next_store_index(&self, keychain: &K) -> u32 {
        self.inner()
            .all_spks()
            // This range is filtering out the spks with a keychain different than
            // `keychain`. We don't use filter here as range is more optimized.
            .range((keychain.clone(), u32::MIN)..(keychain.clone(), u32::MAX))
            .last()
            .map_or(0, |((_, index), _)| *index + 1)
    }

    /// Get an unbounded spk iterator over a given `keychain`.
    ///
    /// # Panics
    ///
    /// This will panic if the given `keychain`'s descriptor does not exist.
    pub fn unbounded_spk_iter(&self, keychain: &K) -> SpkIterator<Descriptor<DescriptorPublicKey>> {
        SpkIterator::new(
            self.keychains
                .get(keychain)
                .expect("keychain does not exist")
                .clone(),
        )
    }

    /// Get unbounded spk iterators for all keychains.
    pub fn all_unbounded_spk_iters(
        &self,
    ) -> BTreeMap<K, SpkIterator<Descriptor<DescriptorPublicKey>>> {
        self.keychains
            .iter()
            .map(|(k, descriptor)| (k.clone(), SpkIterator::new(descriptor.clone())))
            .collect()
    }

    /// Iterate over revealed spks of all keychains.
    pub fn revealed_spks(&self) -> impl DoubleEndedIterator<Item = (K, u32, &Script)> + Clone {
        self.keychains.keys().flat_map(|keychain| {
            self.revealed_keychain_spks(keychain)
                .map(|(i, spk)| (keychain.clone(), i, spk))
        })
    }

    /// Iterate over revealed spks of the given `keychain`.
    pub fn revealed_keychain_spks(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, &Script)> + Clone {
        let next_i = self.last_revealed.get(keychain).map_or(0, |&i| i + 1);
        self.inner
            .all_spks()
            .range((keychain.clone(), u32::MIN)..(keychain.clone(), next_i))
            .map(|((_, i), spk)| (*i, spk.as_script()))
    }

    /// Iterate over revealed, but unused, spks of all keychains.
    pub fn unused_spks(&self) -> impl DoubleEndedIterator<Item = (K, u32, &Script)> + Clone {
        self.keychains.keys().flat_map(|keychain| {
            self.unused_keychain_spks(keychain)
                .map(|(i, spk)| (keychain.clone(), i, spk))
        })
    }

    /// Iterate over revealed, but unused, spks of the given `keychain`.
    pub fn unused_keychain_spks(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, &Script)> + Clone {
        let next_i = self.last_revealed.get(keychain).map_or(0, |&i| i + 1);
        self.inner
            .unused_spks((keychain.clone(), u32::MIN)..(keychain.clone(), next_i))
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
        let descriptor = self.keychains.get(keychain).expect("keychain must exist");
        let last_index = self.last_revealed.get(keychain).cloned();

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
    pub fn last_revealed_indices(&self) -> &BTreeMap<K, u32> {
        &self.last_revealed
    }

    /// Get the last derivation index revealed for `keychain`.
    pub fn last_revealed_index(&self, keychain: &K) -> Option<u32> {
        self.last_revealed.get(keychain).cloned()
    }

    /// Convenience method to call [`Self::reveal_to_target`] on multiple keychains.
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
        let descriptor = self.keychains.get(keychain).expect("keychain must exist");
        let has_wildcard = descriptor.has_wildcard();

        let target_index = if has_wildcard { target_index } else { 0 };
        let next_reveal_index = self
            .last_revealed
            .get(keychain)
            .map_or(0, |index| *index + 1);

        debug_assert!(next_reveal_index + self.lookahead >= self.next_store_index(keychain));

        // If the target_index is already revealed, we are done
        if next_reveal_index > target_index {
            return (
                SpkIterator::new_with_range(
                    descriptor.clone(),
                    next_reveal_index..next_reveal_index,
                ),
                super::ChangeSet::default(),
            );
        }

        // We range over the indexes that are not stored and insert their spks in the index.
        // Indexes from next_reveal_index to next_reveal_index + lookahead are already stored (due
        // to lookahead), so we only range from next_reveal_index + lookahead to target + lookahead
        let range = next_reveal_index + self.lookahead..=target_index + self.lookahead;
        for (new_index, new_spk) in SpkIterator::new_with_range(descriptor, range) {
            let _inserted = self
                .inner
                .insert_spk((keychain.clone(), new_index), new_spk);
            debug_assert!(_inserted, "must not have existing spk");
            debug_assert!(
                has_wildcard || new_index == 0,
                "non-wildcard descriptors must not iterate past index 0"
            );
        }

        let _old_index = self.last_revealed.insert(keychain.clone(), target_index);
        debug_assert!(_old_index < Some(target_index));
        (
            SpkIterator::new_with_range(descriptor.clone(), next_reveal_index..target_index + 1),
            super::ChangeSet(core::iter::once((keychain.clone(), target_index)).collect()),
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
        let (next_index, _) = self.next_index(keychain);
        let changeset = self.reveal_to_target(keychain, next_index).1;
        let script = self
            .inner
            .spk_at_index(&(keychain.clone(), next_index))
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

    /// Iterates over all the [`OutPoint`] that have a `TxOut` with a script pubkey derived from
    /// `keychain`.
    pub fn keychain_outpoints(
        &self,
        keychain: &K,
    ) -> impl DoubleEndedIterator<Item = (u32, OutPoint)> + '_ {
        self.inner
            .outputs_in_range((keychain.clone(), u32::MIN)..(keychain.clone(), u32::MAX))
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
        self.keychains
            .iter()
            .filter_map(|(keychain, _)| {
                self.last_used_index(keychain)
                    .map(|index| (keychain.clone(), index))
            })
            .collect()
    }

    /// Applies the derivation changeset to the [`KeychainTxOutIndex`], extending the number of
    /// derived scripts per keychain, as specified in the `changeset`.
    pub fn apply_changeset(&mut self, changeset: super::ChangeSet<K>) {
        let _ = self.reveal_to_target_multi(&changeset.0);
    }
}
