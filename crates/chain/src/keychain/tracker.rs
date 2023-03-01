use bitcoin::Transaction;
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::{
    chain_graph::{self, ChainGraph},
    collections::*,
    keychain::{KeychainChangeSet, KeychainScan, KeychainTxOutIndex},
    sparse_chain::{self, SparseChain},
    tx_graph::TxGraph,
    AsTransaction, BlockId, FullTxOut, IntoOwned, TxHeight,
};

use super::{Balance, DerivationAdditions};

/// A convenient combination of a [`KeychainTxOutIndex`] and a [`ChainGraph`].
///
/// The [`KeychainTracker`] atomically updates its [`KeychainTxOutIndex`] whenever new chain data is
/// incorporated into its internal [`ChainGraph`].
#[derive(Clone, Debug)]
pub struct KeychainTracker<K, P, T = Transaction> {
    /// Index between script pubkeys to transaction outputs
    pub txout_index: KeychainTxOutIndex<K>,
    chain_graph: ChainGraph<P, T>,
}

impl<K, P, T> KeychainTracker<K, P, T>
where
    P: sparse_chain::ChainPosition,
    K: Ord + Clone + core::fmt::Debug,
    T: AsTransaction + Clone + Ord,
{
    /// Add a keychain to the tracker's `txout_index` with a descriptor to derive addresses for it.
    /// This is just shorthand for calling [`KeychainTxOutIndex::add_keychain`] on the internal
    /// `txout_index`.
    ///
    /// Adding a keychain means you will be able to derive new script pubkeys under that keychain
    /// and the tracker will discover transaction outputs with those script pubkeys.
    pub fn add_keychain(&mut self, keychain: K, descriptor: Descriptor<DescriptorPublicKey>) {
        self.txout_index.add_keychain(keychain, descriptor)
    }

    /// Get the internal map of keychains to their descriptors. This is just shorthand for calling
    /// [`KeychainTxOutIndex::keychains`] on the internal `txout_index`.
    pub fn keychains(&mut self) -> &BTreeMap<K, Descriptor<DescriptorPublicKey>> {
        self.txout_index.keychains()
    }

    /// Get the checkpoint limit of the internal [`SparseChain`].
    ///
    /// Refer to [`SparseChain::checkpoint_limit`] for more.
    pub fn checkpoint_limit(&self) -> Option<usize> {
        self.chain_graph.checkpoint_limit()
    }

    /// Set the checkpoint limit of the internal [`SparseChain`].
    ///
    /// Refer to [`SparseChain::set_checkpoint_limit`] for more.
    pub fn set_checkpoint_limit(&mut self, limit: Option<usize>) {
        self.chain_graph.set_checkpoint_limit(limit)
    }

    /// Determines the resultant [`KeychainChangeSet`] if the given [`KeychainScan`] is applied.
    ///
    /// Internally, we call [`ChainGraph::determine_changeset`] and also determine the additions of
    /// [`KeychainTxOutIndex`].
    pub fn determine_changeset<T2>(
        &self,
        scan: &KeychainScan<K, P, T2>,
    ) -> Result<KeychainChangeSet<K, P, T>, chain_graph::UpdateError<P>>
    where
        T2: IntoOwned<T> + Clone,
    {
        // TODO: `KeychainTxOutIndex::determine_additions`
        let mut derivation_indices = scan.last_active_indices.clone();
        derivation_indices.retain(|keychain, index| {
            match self.txout_index.last_revealed_index(keychain) {
                Some(existing) => *index > existing,
                None => true,
            }
        });

        Ok(KeychainChangeSet {
            derivation_indices: DerivationAdditions(derivation_indices),
            chain_graph: self.chain_graph.determine_changeset(&scan.update)?,
        })
    }

    /// Directly applies a [`KeychainScan`] on [`KeychainTracker`].
    ///
    /// This is equivilant to calling [`determine_changeset`] and [`apply_changeset`] in sequence.
    ///
    /// [`determine_changeset`]: Self::determine_changeset
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn apply_update<T2>(
        &mut self,
        scan: KeychainScan<K, P, T2>,
    ) -> Result<KeychainChangeSet<K, P, T>, chain_graph::UpdateError<P>>
    where
        T2: IntoOwned<T> + Clone,
    {
        let changeset = self.determine_changeset(&scan)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    /// Applies the changes in `changeset` to [`KeychainTracker`].
    ///
    /// Internally, this calls [`KeychainTxOutIndex::apply_additions`] and
    /// [`ChainGraph::apply_changeset`] in sequence.
    pub fn apply_changeset(&mut self, changeset: KeychainChangeSet<K, P, T>) {
        let KeychainChangeSet {
            derivation_indices,
            chain_graph,
        } = changeset;
        self.txout_index.apply_additions(derivation_indices);
        let _ = self.txout_index.scan(&chain_graph);
        self.chain_graph.apply_changeset(chain_graph)
    }

    /// Iterates through [`FullTxOut`]s that are considered to exist in our representation of the
    /// blockchain/mempool.
    ///
    /// In other words, these are `txout`s of confirmed and in-mempool transactions, based on our
    /// view of the blockchain/mempool.
    pub fn full_txouts(&self) -> impl Iterator<Item = (&(K, u32), FullTxOut<P>)> + '_ {
        self.txout_index
            .txouts()
            .filter_map(|(spk_i, op, _)| Some((spk_i, self.chain_graph.full_txout(op)?)))
    }

    /// Iterates through [`FullTxOut`]s that are unspent outputs.
    ///
    /// Refer to [`full_txouts`] for more.
    ///
    /// [`full_txouts`]: Self::full_txouts
    pub fn full_utxos(&self) -> impl Iterator<Item = (&(K, u32), FullTxOut<P>)> + '_ {
        self.full_txouts()
            .filter(|(_, txout)| txout.spent_by.is_none())
    }

    /// Returns a reference to the internal [`ChainGraph`].
    pub fn chain_graph(&self) -> &ChainGraph<P, T> {
        &self.chain_graph
    }

    /// Returns a reference to the internal [`TxGraph`] (which is part of the [`ChainGraph`]).
    pub fn graph(&self) -> &TxGraph<T> {
        &self.chain_graph().graph()
    }

    /// Returns a reference to the internal [`SparseChain`] (which is part of the [`ChainGraph`]).
    pub fn chain(&self) -> &SparseChain<P> {
        &self.chain_graph().chain()
    }

    /// Determines the changes as result of inserting `block_id` (a height and block hash) into the
    /// tracker.
    ///
    /// The caller is responsible for guaranteeing that a block exists at that height. If a
    /// checkpoint already exists at that height with a different hash this will return an error.
    /// Otherwise it will return `Ok(true)` if the checkpoint didn't already exist or `Ok(false)`
    /// if it did.
    ///
    /// **Warning**: This function modifies the internal state of the tracker. You are responsible
    /// for persisting these changes to disk if you need to restore them.
    pub fn insert_checkpoint_preview(
        &self,
        block_id: BlockId,
    ) -> Result<KeychainChangeSet<K, P, T>, chain_graph::InsertCheckpointError> {
        Ok(KeychainChangeSet {
            chain_graph: self.chain_graph.insert_checkpoint_preview(block_id)?,
            ..Default::default()
        })
    }

    /// Directly insert a `block_id` into the tracker.
    ///
    /// This is equivalent of calling [`insert_checkpoint_preview`] and [`apply_changeset`] in
    /// sequence.
    ///
    /// [`insert_checkpoint_preview`]: Self::insert_checkpoint_preview
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn insert_checkpoint(
        &mut self,
        block_id: BlockId,
    ) -> Result<KeychainChangeSet<K, P, T>, chain_graph::InsertCheckpointError> {
        let changeset = self.insert_checkpoint_preview(block_id)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    /// Determines the changes as result of inserting a transaction into the inner [`ChainGraph`]
    /// and optionally into the inner chain at `position`.
    ///
    /// **Warning**: This function modifies the internal state of the chain graph. You are
    /// responsible for persisting these changes to disk if you need to restore them.
    pub fn insert_tx_preview(
        &self,
        tx: T,
        pos: P,
    ) -> Result<KeychainChangeSet<K, P, T>, chain_graph::InsertTxError<P>> {
        Ok(KeychainChangeSet {
            chain_graph: self.chain_graph.insert_tx_preview(tx, pos)?,
            ..Default::default()
        })
    }

    /// Directly insert a transaction into the inner [`ChainGraph`] and optionally into the inner
    /// chain at `position`.
    ///
    /// This is equivilant of calling [`insert_tx_preview`] and [`apply_changeset`] in sequence.
    ///
    /// [`insert_tx_preview`]: Self::insert_tx_preview
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn insert_tx(
        &mut self,
        tx: T,
        pos: P,
    ) -> Result<KeychainChangeSet<K, P, T>, chain_graph::InsertTxError<P>> {
        let changeset = self.insert_tx_preview(tx, pos)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    /// Returns the *balance* of the keychain i.e. the value of unspent transaction outputs tracked.
    ///
    /// The caller provides a `should_trust` predicate which must decide whether the value of
    /// unconfirmed outputs on this keychain are guaranteed to be realized or not. For example:
    ///
    /// - For an *internal* (change) keychain `should_trust` should in general be `true` since even if
    /// you lose an internal output due to eviction you will always gain back the value from whatever output the
    /// unconfirmed transaction was spending (since that output is presumeably from your wallet).
    /// - For an *external* keychain you might want `should_trust` to return  `false` since someone may cancel (by double spending)
    /// a payment made to addresses on that keychain.
    ///
    /// When in doubt set `should_trust` to return false. This doesn't do anything other than change
    /// where the unconfirmed output's value is accounted for in `Balance`.
    pub fn balance(&self, mut should_trust: impl FnMut(&K) -> bool) -> Balance {
        let mut immature = 0;
        let mut trusted_pending = 0;
        let mut untrusted_pending = 0;
        let mut confirmed = 0;
        let last_sync_height = self.chain().latest_checkpoint().map(|latest| latest.height);
        for ((keychain, _), utxo) in self.full_utxos() {
            let chain_position = &utxo.chain_position;

            match chain_position.height() {
                TxHeight::Confirmed(_) => {
                    if utxo.is_on_coinbase {
                        if utxo.is_mature(
                            last_sync_height
                                .expect("since it's confirmed we must have a checkpoint"),
                        ) {
                            confirmed += utxo.txout.value;
                        } else {
                            immature += utxo.txout.value;
                        }
                    } else {
                        confirmed += utxo.txout.value;
                    }
                }
                TxHeight::Unconfirmed => {
                    if should_trust(keychain) {
                        trusted_pending += utxo.txout.value;
                    } else {
                        untrusted_pending += utxo.txout.value;
                    }
                }
            }
        }

        Balance {
            immature,
            trusted_pending,
            untrusted_pending,
            confirmed,
        }
    }

    /// Returns the balance of all spendable confirmed unspent outputs of this tracker at a
    /// particular height.
    pub fn balance_at(&self, height: u32) -> u64 {
        self.full_txouts()
            .filter(|(_, full_txout)| full_txout.is_spendable_at(height))
            .map(|(_, full_txout)| full_txout.txout.value)
            .sum()
    }
}

impl<K, P> Default for KeychainTracker<K, P> {
    fn default() -> Self {
        Self {
            txout_index: Default::default(),
            chain_graph: Default::default(),
        }
    }
}

impl<K, P> AsRef<SparseChain<P>> for KeychainTracker<K, P> {
    fn as_ref(&self) -> &SparseChain<P> {
        self.chain_graph.chain()
    }
}

impl<K, P> AsRef<TxGraph> for KeychainTracker<K, P> {
    fn as_ref(&self) -> &TxGraph {
        self.chain_graph.graph()
    }
}

impl<K, P> AsRef<ChainGraph<P>> for KeychainTracker<K, P> {
    fn as_ref(&self) -> &ChainGraph<P> {
        &self.chain_graph
    }
}
