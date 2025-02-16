//! [`KeychainTxOutIndex`] controls how script pubkeys are revealed for multiple keychains and
//! indexes [`TxOut`]s with them.

use crate::{
    collections::*,
    miniscript::{Descriptor, DescriptorPublicKey},
    spk_client::{FullScanRequestBuilder, SyncRequestBuilder},
    spk_iter::BIP32_MAX_INDEX,
    spk_txout::SpkTxOutIndex,
    DescriptorExt, DescriptorId, Indexed, Indexer, KeychainIndexed, SpkIterator,
};
use alloc::{borrow::ToOwned, vec::Vec};
use bitcoin::{Amount, OutPoint, ScriptBuf, SignedAmount, Transaction, TxOut, Txid};
use core::{
    fmt::Debug,
    ops::{Bound, RangeBounds},
};

use crate::Merge;

/// The default lookahead for a [`KeychainTxOutIndex`]
pub const DEFAULT_LOOKAHEAD: u32 = 25;

/// [`KeychainTxOutIndex`] controls how script pubkeys are revealed for multiple keychains, and
/// indexes [`TxOut`]s with them.
///
/// A single keychain is a chain of script pubkeys derived from a single [`Descriptor`]. Keychains
/// are identified using the `K` generic. Script pubkeys are identified by the keychain that they
/// are derived from `K`, as well as the derivation index `u32`.
///
/// There is a strict 1-to-1 relationship between descriptors and keychains. Each keychain has one
/// and only one descriptor and each descriptor has one and only one keychain. The
/// [`insert_descriptor`] method will return an error if you try and violate this invariant. This
/// rule is a proxy for a stronger rule: no two descriptors should produce the same script pubkey.
/// Having two descriptors produce the same script pubkey should cause whichever keychain derives
/// the script pubkey first to be the effective owner of it but you should not rely on this
/// behaviour. ⚠ It is up you, the developer, not to violate this invariant.
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
/// The [`KeychainTxOutIndex`] is constructed with the `lookahead` and cannot be altered. See
/// [`DEFAULT_LOOKAHEAD`] for the value used in the `Default` implementation. Use [`new`] to set a
/// custom `lookahead`.
///
/// # Unbounded script pubkey iterator
///
/// For script-pubkey-based chain sources (such as Electrum/Esplora), an initial scan is best done
/// by iterating though derived script pubkeys one by one and requesting transaction histories for
/// each script pubkey. We will stop after x-number of script pubkeys have empty histories. An
/// unbounded script pubkey iterator is useful to pass to such a chain source because it doesn't
/// require holding a reference to the index.
///
/// Call [`unbounded_spk_iter`] to get an unbounded script pubkey iterator for a given keychain.
/// Call [`all_unbounded_spk_iters`] to get unbounded script pubkey iterators for all keychains.
///
/// # Change sets
///
/// Methods that can update the last revealed index or add keychains will return [`ChangeSet`] to report
/// these changes. This should be persisted for future recovery.
///
/// ## Synopsis
///
/// ```
/// use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
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
/// let _ = txout_index.insert_descriptor(MyKeychain::External, external_descriptor)?;
/// let _ = txout_index.insert_descriptor(MyKeychain::Internal, internal_descriptor)?;
/// let _ = txout_index.insert_descriptor(MyKeychain::MyAppUser { user_id: 42 }, descriptor_42)?;
///
/// let new_spk_for_user = txout_index.reveal_next_spk(MyKeychain::MyAppUser{ user_id: 42 });
/// # Ok::<_, bdk_chain::indexer::keychain_txout::InsertDescriptorError<_>>(())
/// ```
///
/// [`Ord`]: core::cmp::Ord
/// [`SpkTxOutIndex`]: crate::spk_txout::SpkTxOutIndex
/// [`Descriptor`]: crate::miniscript::Descriptor
/// [`reveal_to_target`]: Self::reveal_to_target
/// [`reveal_next_spk`]: Self::reveal_next_spk
/// [`revealed_keychain_spks`]: Self::revealed_keychain_spks
/// [`revealed_spks`]: Self::revealed_spks
/// [`index_tx`]: Self::index_tx
/// [`index_txout`]: Self::index_txout
/// [`new`]: Self::new
/// [`unbounded_spk_iter`]: Self::unbounded_spk_iter
/// [`all_unbounded_spk_iters`]: Self::all_unbounded_spk_iters
/// [`outpoints`]: Self::outpoints
/// [`txouts`]: Self::txouts
/// [`unused_spks`]: Self::unused_spks
/// [`insert_descriptor`]: Self::insert_descriptor
#[derive(Clone, Debug)]
pub struct KeychainTxOutIndex<K> {
    inner: SpkTxOutIndex<(K, u32)>,
    keychain_to_descriptor_id: BTreeMap<K, DescriptorId>,
    descriptor_id_to_keychain: HashMap<DescriptorId, K>,
    descriptors: HashMap<DescriptorId, Descriptor<DescriptorPublicKey>>,
    last_revealed: HashMap<DescriptorId, u32>,
    lookahead: u32,
}

impl<K> Default for KeychainTxOutIndex<K> {
    fn default() -> Self {
        Self::new(DEFAULT_LOOKAHEAD)
    }
}

impl<K> AsRef<SpkTxOutIndex<(K, u32)>> for KeychainTxOutIndex<K> {
    fn as_ref(&self) -> &SpkTxOutIndex<(K, u32)> {
        &self.inner
    }
}

impl<K: Clone + Ord + Debug> Indexer for KeychainTxOutIndex<K> {
    type ChangeSet = ChangeSet;

    fn index_txout(&mut self, outpoint: OutPoint, txout: &TxOut) -> Self::ChangeSet {
        let mut changeset = ChangeSet::default();
        if let Some((keychain, index)) = self.inner.scan_txout(outpoint, txout).cloned() {
            let did = self
                .keychain_to_descriptor_id
                .get(&keychain)
                .expect("invariant");
            if self.last_revealed.get(did) < Some(&index) {
                self.last_revealed.insert(*did, index);
                changeset.last_revealed.insert(*did, index);
                self.replenish_inner_index(*did, &keychain, self.lookahead);
            }
        }
        changeset
    }

    fn index_tx(&mut self, tx: &bitcoin::Transaction) -> Self::ChangeSet {
        let mut changeset = ChangeSet::default();
        let txid = tx.compute_txid();
        for (op, txout) in tx.output.iter().enumerate() {
            changeset.merge(self.index_txout(OutPoint::new(txid, op as u32), txout));
        }
        changeset
    }

    fn initial_changeset(&self) -> Self::ChangeSet {
        ChangeSet {
            last_revealed: self.last_revealed.clone().into_iter().collect(),
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
            keychain_to_descriptor_id: Default::default(),
            descriptors: Default::default(),
            descriptor_id_to_keychain: Default::default(),
            last_revealed: Default::default(),
            lookahead,
        }
    }

    /// Get a reference to the internal [`SpkTxOutIndex`].
    pub fn inner(&self) -> &SpkTxOutIndex<(K, u32)> {
        &self.inner
    }
}

/// Methods that are *re-exposed* from the internal [`SpkTxOutIndex`].
impl<K: Clone + Ord + Debug> KeychainTxOutIndex<K> {
    /// Get the set of indexed outpoints, corresponding to tracked keychains.
    pub fn outpoints(&self) -> &BTreeSet<KeychainIndexed<K, OutPoint>> {
        self.inner.outpoints()
    }

    /// Iterate over known txouts that spend to tracked script pubkeys.
    pub fn txouts(
        &self,
    ) -> impl DoubleEndedIterator<Item = KeychainIndexed<K, (OutPoint, &TxOut)>> + ExactSizeIterator
    {
        self.inner
            .txouts()
            .map(|(index, op, txout)| (index.clone(), (op, txout)))
    }

    /// Finds all txouts on a transaction that has previously been scanned and indexed.
    pub fn txouts_in_tx(
        &self,
        txid: Txid,
    ) -> impl DoubleEndedIterator<Item = KeychainIndexed<K, (OutPoint, &TxOut)>> {
        self.inner
            .txouts_in_tx(txid)
            .map(|(index, op, txout)| (index.clone(), (op, txout)))
    }

    /// Return the [`TxOut`] of `outpoint` if it has been indexed, and if it corresponds to a
    /// tracked keychain.
    ///
    /// The associated keychain and keychain index of the txout's spk is also returned.
    ///
    /// This calls [`SpkTxOutIndex::txout`] internally.
    pub fn txout(&self, outpoint: OutPoint) -> Option<KeychainIndexed<K, &TxOut>> {
        self.inner
            .txout(outpoint)
            .map(|(index, txout)| (index.clone(), txout))
    }

    /// Return the script that exists under the given `keychain`'s `index`.
    ///
    /// This calls [`SpkTxOutIndex::spk_at_index`] internally.
    pub fn spk_at_index(&self, keychain: K, index: u32) -> Option<ScriptBuf> {
        self.inner.spk_at_index(&(keychain.clone(), index))
    }

    /// Returns the keychain and keychain index associated with the spk.
    ///
    /// This calls [`SpkTxOutIndex::index_of_spk`] internally.
    pub fn index_of_spk(&self, script: ScriptBuf) -> Option<&(K, u32)> {
        self.inner.index_of_spk(script)
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
    /// Return all keychains and their corresponding descriptors.
    pub fn keychains(
        &self,
    ) -> impl DoubleEndedIterator<Item = (K, &Descriptor<DescriptorPublicKey>)> + ExactSizeIterator + '_
    {
        self.keychain_to_descriptor_id
            .iter()
            .map(|(k, did)| (k.clone(), self.descriptors.get(did).expect("invariant")))
    }

    /// Insert a descriptor with a keychain associated to it.
    ///
    /// Adding a descriptor means you will be able to derive new script pubkeys under it and the
    /// txout index will discover transaction outputs with those script pubkeys (once they've been
    /// derived and added to the index).
    ///
    /// keychain <-> descriptor is a one-to-one mapping that cannot be changed. Attempting to do so
    /// will return a [`InsertDescriptorError<K>`].
    ///
    /// [`KeychainTxOutIndex`] will prevent you from inserting two descriptors which derive the same
    /// script pubkey at index 0, but it's up to you to ensure that descriptors don't collide at
    /// other indices. If they do nothing catastrophic happens at the `KeychainTxOutIndex` level
    /// (one keychain just becomes the defacto owner of that spk arbitrarily) but this may have
    /// subtle implications up the application stack like one UTXO being missing from one keychain
    /// because it has been assigned to another which produces the same script pubkey.
    pub fn insert_descriptor(
        &mut self,
        keychain: K,
        descriptor: Descriptor<DescriptorPublicKey>,
    ) -> Result<bool, InsertDescriptorError<K>> {
        let did = descriptor.descriptor_id();
        if !self.keychain_to_descriptor_id.contains_key(&keychain)
            && !self.descriptor_id_to_keychain.contains_key(&did)
        {
            self.descriptors.insert(did, descriptor.clone());
            self.keychain_to_descriptor_id.insert(keychain.clone(), did);
            self.descriptor_id_to_keychain.insert(did, keychain.clone());
            self.replenish_inner_index(did, &keychain, self.lookahead);
            return Ok(true);
        }

        if let Some(existing_desc_id) = self.keychain_to_descriptor_id.get(&keychain) {
            let descriptor = self.descriptors.get(existing_desc_id).expect("invariant");
            if *existing_desc_id != did {
                return Err(InsertDescriptorError::KeychainAlreadyAssigned {
                    existing_assignment: descriptor.clone(),
                    keychain,
                });
            }
        }

        if let Some(existing_keychain) = self.descriptor_id_to_keychain.get(&did) {
            let descriptor = self.descriptors.get(&did).expect("invariant").clone();

            if *existing_keychain != keychain {
                return Err(InsertDescriptorError::DescriptorAlreadyAssigned {
                    existing_assignment: existing_keychain.clone(),
                    descriptor,
                });
            }
        }

        Ok(false)
    }

    /// Gets the descriptor associated with the keychain. Returns `None` if the keychain doesn't
    /// have a descriptor associated with it.
    pub fn get_descriptor(&self, keychain: K) -> Option<&Descriptor<DescriptorPublicKey>> {
        let did = self.keychain_to_descriptor_id.get(&keychain)?;
        self.descriptors.get(did)
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
    pub fn lookahead_to_target(&mut self, keychain: K, target_index: u32) {
        if let Some((next_index, _)) = self.next_index(keychain.clone()) {
            let temp_lookahead = (target_index + 1)
                .checked_sub(next_index)
                .filter(|&index| index > 0);

            if let Some(temp_lookahead) = temp_lookahead {
                self.replenish_inner_index_keychain(keychain, temp_lookahead);
            }
        }
    }

    fn replenish_inner_index_did(&mut self, did: DescriptorId, lookahead: u32) {
        if let Some(keychain) = self.descriptor_id_to_keychain.get(&did).cloned() {
            self.replenish_inner_index(did, &keychain, lookahead);
        }
    }

    fn replenish_inner_index_keychain(&mut self, keychain: K, lookahead: u32) {
        if let Some(did) = self.keychain_to_descriptor_id.get(&keychain) {
            self.replenish_inner_index(*did, &keychain, lookahead);
        }
    }

    /// Syncs the state of the inner spk index after changes to a keychain
    fn replenish_inner_index(&mut self, did: DescriptorId, keychain: &K, lookahead: u32) {
        let descriptor = self.descriptors.get(&did).expect("invariant");
        let next_store_index = self
            .inner
            .all_spks()
            .range(&(keychain.clone(), u32::MIN)..=&(keychain.clone(), u32::MAX))
            .last()
            .map_or(0, |((_, index), _)| *index + 1);
        let next_reveal_index = self.last_revealed.get(&did).map_or(0, |v| *v + 1);
        for (new_index, new_spk) in
            SpkIterator::new_with_range(descriptor, next_store_index..next_reveal_index + lookahead)
        {
            let _inserted = self
                .inner
                .insert_spk((keychain.clone(), new_index), new_spk);
            debug_assert!(_inserted, "replenish lookahead: must not have existing spk: keychain={:?}, lookahead={}, next_store_index={}, next_reveal_index={}", keychain, lookahead, next_store_index, next_reveal_index);
        }
    }

    /// Get an unbounded spk iterator over a given `keychain`. Returns `None` if the provided
    /// keychain doesn't exist
    pub fn unbounded_spk_iter(
        &self,
        keychain: K,
    ) -> Option<SpkIterator<Descriptor<DescriptorPublicKey>>> {
        let descriptor = self.get_descriptor(keychain)?.clone();
        Some(SpkIterator::new(descriptor))
    }

    /// Get unbounded spk iterators for all keychains.
    pub fn all_unbounded_spk_iters(
        &self,
    ) -> BTreeMap<K, SpkIterator<Descriptor<DescriptorPublicKey>>> {
        self.keychain_to_descriptor_id
            .iter()
            .map(|(k, did)| {
                (
                    k.clone(),
                    SpkIterator::new(self.descriptors.get(did).expect("invariant").clone()),
                )
            })
            .collect()
    }

    /// Iterate over revealed spks of keychains in `range`
    pub fn revealed_spks(
        &self,
        range: impl RangeBounds<K>,
    ) -> impl Iterator<Item = KeychainIndexed<K, ScriptBuf>> + '_ {
        let start = range.start_bound();
        let end = range.end_bound();
        let mut iter_last_revealed = self
            .keychain_to_descriptor_id
            .range((start, end))
            .map(|(k, did)| (k, self.last_revealed.get(did).cloned()));
        let mut iter_spks = self
            .inner
            .all_spks()
            .range(self.map_to_inner_bounds((start, end)));
        let mut current_keychain = iter_last_revealed.next();
        // The reason we need a tricky algorithm is because of the "lookahead" feature which means
        // that some of the spks in the SpkTxoutIndex will not have been revealed yet. So we need to
        // filter out those spks that are above the last_revealed for that keychain. To do this we
        // iterate through the last_revealed for each keychain and the spks for each keychain in
        // tandem. This minimizes BTreeMap queries.
        core::iter::from_fn(move || loop {
            let ((keychain, index), spk) = iter_spks.next()?;
            // We need to find the last revealed that matches the current spk we are considering so
            // we skip ahead.
            while current_keychain?.0 < keychain {
                current_keychain = iter_last_revealed.next();
            }
            let (current_keychain, last_revealed) = current_keychain?;

            if current_keychain == keychain && Some(*index) <= last_revealed {
                break Some(((keychain.clone(), *index), spk.clone()));
            }
        })
    }

    /// Iterate over revealed spks of the given `keychain` with ascending indices.
    ///
    /// This is a double ended iterator so you can easily reverse it to get an iterator where
    /// the script pubkeys that were most recently revealed are first.
    pub fn revealed_keychain_spks(
        &self,
        keychain: K,
    ) -> impl DoubleEndedIterator<Item = Indexed<ScriptBuf>> + '_ {
        let end = self
            .last_revealed_index(keychain.clone())
            .map(|v| v + 1)
            .unwrap_or(0);
        self.inner
            .all_spks()
            .range((keychain.clone(), 0)..(keychain.clone(), end))
            .map(|((_, index), spk)| (*index, spk.clone()))
    }

    /// Iterate over revealed, but unused, spks of all keychains.
    pub fn unused_spks(
        &self,
    ) -> impl DoubleEndedIterator<Item = KeychainIndexed<K, ScriptBuf>> + Clone + '_ {
        self.keychain_to_descriptor_id.keys().flat_map(|keychain| {
            self.unused_keychain_spks(keychain.clone())
                .map(|(i, spk)| ((keychain.clone(), i), spk.clone()))
        })
    }

    /// Iterate over revealed, but unused, spks of the given `keychain`.
    /// Returns an empty iterator if the provided keychain doesn't exist.
    pub fn unused_keychain_spks(
        &self,
        keychain: K,
    ) -> impl DoubleEndedIterator<Item = Indexed<ScriptBuf>> + Clone + '_ {
        let end = match self.keychain_to_descriptor_id.get(&keychain) {
            Some(did) => self.last_revealed.get(did).map(|v| *v + 1).unwrap_or(0),
            None => 0,
        };

        self.inner
            .unused_spks((keychain.clone(), 0)..(keychain.clone(), end))
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
    pub fn next_index(&self, keychain: K) -> Option<(u32, bool)> {
        let did = self.keychain_to_descriptor_id.get(&keychain)?;
        let last_index = self.last_revealed.get(did).cloned();
        let descriptor = self.descriptors.get(did).expect("invariant");

        // we can only get the next index if the wildcard exists.
        let has_wildcard = descriptor.has_wildcard();

        Some(match last_index {
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
        })
    }

    /// Get the last derivation index that is revealed for each keychain.
    ///
    /// Keychains with no revealed indices will not be included in the returned [`BTreeMap`].
    pub fn last_revealed_indices(&self) -> BTreeMap<K, u32> {
        self.last_revealed
            .iter()
            .filter_map(|(desc_id, index)| {
                let keychain = self.descriptor_id_to_keychain.get(desc_id)?;
                Some((keychain.clone(), *index))
            })
            .collect()
    }

    /// Get the last derivation index revealed for `keychain`. Returns None if the keychain doesn't
    /// exist, or if the keychain doesn't have any revealed scripts.
    pub fn last_revealed_index(&self, keychain: K) -> Option<u32> {
        let descriptor_id = self.keychain_to_descriptor_id.get(&keychain)?;
        self.last_revealed.get(descriptor_id).cloned()
    }

    /// Convenience method to call [`Self::reveal_to_target`] on multiple keychains.
    pub fn reveal_to_target_multi(&mut self, keychains: &BTreeMap<K, u32>) -> ChangeSet {
        let mut changeset = ChangeSet::default();

        for (keychain, &index) in keychains {
            if let Some((_, new_changeset)) = self.reveal_to_target(keychain.clone(), index) {
                changeset.merge(new_changeset);
            }
        }

        changeset
    }

    /// Reveals script pubkeys of the `keychain`'s descriptor **up to and including** the
    /// `target_index`.
    ///
    /// If the `target_index` cannot be reached (due to the descriptor having no wildcard and/or
    /// the `target_index` is in the hardened index range), this method will make a best-effort and
    /// reveal up to the last possible index.
    ///
    /// This returns list of newly revealed indices (alongside their scripts) and a
    /// [`ChangeSet`], which reports updates to the latest revealed index. If no new script
    /// pubkeys are revealed, then both of these will be empty.
    ///
    /// Returns None if the provided `keychain` doesn't exist.
    #[must_use]
    pub fn reveal_to_target(
        &mut self,
        keychain: K,
        target_index: u32,
    ) -> Option<(Vec<Indexed<ScriptBuf>>, ChangeSet)> {
        let mut changeset = ChangeSet::default();
        let mut spks: Vec<Indexed<ScriptBuf>> = vec![];
        while let Some((i, new)) = self.next_index(keychain.clone()) {
            if !new || i > target_index {
                break;
            }
            match self.reveal_next_spk(keychain.clone()) {
                Some(((i, spk), change)) => {
                    spks.push((i, spk));
                    changeset.merge(change);
                }
                None => break,
            }
        }

        Some((spks, changeset))
    }

    /// Attempts to reveal the next script pubkey for `keychain`.
    ///
    /// Returns the derivation index of the revealed script pubkey, the revealed script pubkey and a
    /// [`ChangeSet`] which represents changes in the last revealed index (if any).
    /// Returns None if the provided keychain doesn't exist.
    ///
    /// When a new script cannot be revealed, we return the last revealed script and an empty
    /// [`ChangeSet`]. There are two scenarios when a new script pubkey cannot be derived:
    ///
    ///  1. The descriptor has no wildcard and already has one script revealed.
    ///  2. The descriptor has already revealed scripts up to the numeric bound.
    ///  3. There is no descriptor associated with the given keychain.
    pub fn reveal_next_spk(&mut self, keychain: K) -> Option<(Indexed<ScriptBuf>, ChangeSet)> {
        let (next_index, new) = self.next_index(keychain.clone())?;
        let mut changeset = ChangeSet::default();

        if new {
            let did = self.keychain_to_descriptor_id.get(&keychain)?;
            self.last_revealed.insert(*did, next_index);
            changeset.last_revealed.insert(*did, next_index);
            self.replenish_inner_index(*did, &keychain, self.lookahead);
        }
        let script = self
            .inner
            .spk_at_index(&(keychain.clone(), next_index))
            .expect("we just inserted it");
        Some(((next_index, script), changeset))
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
    /// Returns `None` if there are no script pubkeys that have been used and no new script pubkey
    /// could be revealed (see [`reveal_next_spk`] for when this happens).
    ///
    /// [`reveal_next_spk`]: Self::reveal_next_spk
    pub fn next_unused_spk(&mut self, keychain: K) -> Option<(Indexed<ScriptBuf>, ChangeSet)> {
        let next_unused = self
            .unused_keychain_spks(keychain.clone())
            .next()
            .map(|(i, spk)| ((i, spk.to_owned()), ChangeSet::default()));

        next_unused.or_else(|| self.reveal_next_spk(keychain))
    }

    /// Iterate over all [`OutPoint`]s that have `TxOut`s with script pubkeys derived from
    /// `keychain`.
    pub fn keychain_outpoints(
        &self,
        keychain: K,
    ) -> impl DoubleEndedIterator<Item = Indexed<OutPoint>> + '_ {
        self.keychain_outpoints_in_range(keychain.clone()..=keychain)
            .map(|((_, i), op)| (i, op))
    }

    /// Iterate over [`OutPoint`]s that have script pubkeys derived from keychains in `range`.
    pub fn keychain_outpoints_in_range<'a>(
        &'a self,
        range: impl RangeBounds<K> + 'a,
    ) -> impl DoubleEndedIterator<Item = KeychainIndexed<K, OutPoint>> + 'a {
        self.inner
            .outputs_in_range(self.map_to_inner_bounds(range))
            .map(|((k, i), op)| ((k.clone(), *i), op))
    }

    fn map_to_inner_bounds(&self, bound: impl RangeBounds<K>) -> impl RangeBounds<(K, u32)> {
        let start = match bound.start_bound() {
            Bound::Included(keychain) => Bound::Included((keychain.clone(), u32::MIN)),
            Bound::Excluded(keychain) => Bound::Excluded((keychain.clone(), u32::MAX)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let end = match bound.end_bound() {
            Bound::Included(keychain) => Bound::Included((keychain.clone(), u32::MAX)),
            Bound::Excluded(keychain) => Bound::Excluded((keychain.clone(), u32::MIN)),
            Bound::Unbounded => Bound::Unbounded,
        };

        (start, end)
    }

    /// Returns the highest derivation index of the `keychain` where [`KeychainTxOutIndex`] has
    /// found a [`TxOut`] with it's script pubkey.
    pub fn last_used_index(&self, keychain: K) -> Option<u32> {
        self.keychain_outpoints(keychain).last().map(|(i, _)| i)
    }

    /// Returns the highest derivation index of each keychain that [`KeychainTxOutIndex`] has found
    /// a [`TxOut`] with it's script pubkey.
    pub fn last_used_indices(&self) -> BTreeMap<K, u32> {
        self.keychain_to_descriptor_id
            .iter()
            .filter_map(|(keychain, _)| {
                self.last_used_index(keychain.clone())
                    .map(|index| (keychain.clone(), index))
            })
            .collect()
    }

    /// Applies the `ChangeSet<K>` to the [`KeychainTxOutIndex<K>`]
    pub fn apply_changeset(&mut self, changeset: ChangeSet) {
        for (&desc_id, &index) in &changeset.last_revealed {
            let v = self.last_revealed.entry(desc_id).or_default();
            *v = index.max(*v);
            self.replenish_inner_index_did(desc_id, self.lookahead);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
/// Error returned from [`KeychainTxOutIndex::insert_descriptor`]
pub enum InsertDescriptorError<K> {
    /// The descriptor has already been assigned to a keychain so you can't assign it to another
    DescriptorAlreadyAssigned {
        /// The descriptor you have attempted to reassign
        descriptor: Descriptor<DescriptorPublicKey>,
        /// The keychain that the descriptor is already assigned to
        existing_assignment: K,
    },
    /// The keychain is already assigned to a descriptor so you can't reassign it
    KeychainAlreadyAssigned {
        /// The keychain that you have attempted to reassign
        keychain: K,
        /// The descriptor that the keychain is already assigned to
        existing_assignment: Descriptor<DescriptorPublicKey>,
    },
}

impl<K: core::fmt::Debug> core::fmt::Display for InsertDescriptorError<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InsertDescriptorError::DescriptorAlreadyAssigned {
                existing_assignment: existing,
                descriptor,
            } => {
                write!(
                    f,
                    "attempt to re-assign descriptor {descriptor:?} already assigned to {existing:?}"
                )
            }
            InsertDescriptorError::KeychainAlreadyAssigned {
                existing_assignment: existing,
                keychain,
            } => {
                write!(
                    f,
                    "attempt to re-assign keychain {keychain:?} already assigned to {existing:?}"
                )
            }
        }
    }
}

#[cfg(feature = "std")]
impl<K: core::fmt::Debug> std::error::Error for InsertDescriptorError<K> {}

/// Represents updates to the derivation index of a [`KeychainTxOutIndex`].
/// It maps each keychain `K` to a descriptor and its last revealed index.
///
/// It can be applied to [`KeychainTxOutIndex`] with [`apply_changeset`].
///
/// The `last_revealed` field is monotone in that [`merge`] will never decrease it.
/// `keychains_added` is *not* monotone, once it is set any attempt to change it is subject to the
/// same *one-to-one* keychain <-> descriptor mapping invariant as [`KeychainTxOutIndex`] itself.
///
/// [`KeychainTxOutIndex`]: crate::keychain_txout::KeychainTxOutIndex
/// [`apply_changeset`]: crate::keychain_txout::KeychainTxOutIndex::apply_changeset
/// [`merge`]: Self::merge
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[must_use]
pub struct ChangeSet {
    /// Contains for each descriptor_id the last revealed index of derivation
    pub last_revealed: BTreeMap<DescriptorId, u32>,
}

impl Merge for ChangeSet {
    /// Merge another [`ChangeSet`] into self.
    fn merge(&mut self, other: Self) {
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
        self.last_revealed.is_empty()
    }
}

/// Trait to extend [`SyncRequestBuilder`].
pub trait SyncRequestBuilderExt<K> {
    /// Add [`Script`](bitcoin::Script)s that are revealed by the `indexer` of the given `spk_range`
    /// that will be synced against.
    fn revealed_spks_from_indexer<R>(self, indexer: &KeychainTxOutIndex<K>, spk_range: R) -> Self
    where
        R: core::ops::RangeBounds<K>;

    /// Add [`Script`](bitcoin::Script)s that are revealed by the `indexer` but currently unused.
    fn unused_spks_from_indexer(self, indexer: &KeychainTxOutIndex<K>) -> Self;
}

impl<K: Clone + Ord + core::fmt::Debug> SyncRequestBuilderExt<K> for SyncRequestBuilder<(K, u32)> {
    fn revealed_spks_from_indexer<R>(self, indexer: &KeychainTxOutIndex<K>, spk_range: R) -> Self
    where
        R: core::ops::RangeBounds<K>,
    {
        self.spks_with_indexes(indexer.revealed_spks(spk_range))
    }

    fn unused_spks_from_indexer(self, indexer: &KeychainTxOutIndex<K>) -> Self {
        self.spks_with_indexes(indexer.unused_spks())
    }
}

/// Trait to extend [`FullScanRequestBuilder`].
pub trait FullScanRequestBuilderExt<K> {
    /// Add spk iterators for each keychain tracked in `indexer`.
    fn spks_from_indexer(self, indexer: &KeychainTxOutIndex<K>) -> Self;
}

impl<K: Clone + Ord + core::fmt::Debug> FullScanRequestBuilderExt<K> for FullScanRequestBuilder<K> {
    fn spks_from_indexer(mut self, indexer: &KeychainTxOutIndex<K>) -> Self {
        for (keychain, spks) in indexer.all_unbounded_spk_iters() {
            self = self.spks_for_keychain(keychain, spks);
        }
        self
    }
}
