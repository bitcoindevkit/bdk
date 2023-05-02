use core::{convert::Infallible, fmt::Debug};

use alloc::collections::BTreeMap;
use bitcoin::{BlockHash, Transaction, Txid};
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::{
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{Balance, DerivationAdditions, KeychainTxOutIndex},
    local_chain::{self, LocalChain},
    remote_chain::{self, RemoteChain},
    tx_graph::{CanonicalTx, TxGraph},
    Anchor, Append, BlockId, ChainOracle, FullTxOut, ObservedAs,
};

/// An trait which represents the last seen chain-source state.
///
/// Only [`RemoteChain`] and [`LocalChain`] should implement this.
pub trait LastSeenBlock {
    /// Get the last seen block height and hash.
    fn last_seen_block(&self) -> Option<BlockId>;
}

pub type LocalTracker<K, A> = Tracker<K, A, LocalChain>;
pub type RemoteTracker<K, A, O> = Tracker<K, A, RemoteChain<O>>;

pub type LocalUpdate<K, A> = Update<K, A, LocalChain>;
pub type RemoteUpdate<K, A> = Update<K, A, Option<BlockId>>;

pub type LocalChangeSet<K, A> = ChangeSet<K, A, local_chain::ChangeSet>;
pub type RemoteChangeSet<K, A> = ChangeSet<K, A, remote_chain::ChangeSet>;

/// An in-memory representation of chain data that we are tracking.
///
/// * `K` is our keychain identifier.
/// * `A` is the [`Anchor`] implementation.
/// * `B` is the representation of the best chain history. This can either be a [`LocalChain`] or a
/// [`RemoteChain`] (which wraps a remote [`ChainOracle`] implementation).
///
/// [`Tracker`] can be constructed with [`new_local`] or [`new_remote`] (depending on the
/// chain-history type).
///
/// [`new_local`]: Self::new_local
/// [`new_remote`]: Self::new_remote
#[derive(Debug)]
pub struct Tracker<K, A, C> {
    indexed_graph: IndexedTxGraph<A, KeychainTxOutIndex<K>>,
    chain: C,
    genesis_blockhash: BlockHash,
}

impl<K: Debug + Clone + Ord, A: Anchor> LocalTracker<K, A> {
    pub fn new_local(
        genesis_blockhash: BlockHash,
        keychains: impl IntoIterator<Item = (K, Descriptor<DescriptorPublicKey>)>,
    ) -> Self {
        Self {
            genesis_blockhash,
            indexed_graph: {
                let mut indexed_graph = IndexedTxGraph::<A, KeychainTxOutIndex<K>>::default();
                for (keychain, descriptor) in keychains.into_iter() {
                    indexed_graph.index.add_keychain(keychain, descriptor);
                }
                indexed_graph
            },
            chain: LocalChain::default(),
        }
    }

    pub fn apply_update(
        &mut self,
        update: LocalUpdate<K, A>,
    ) -> (
        LocalChangeSet<K, A>,
        Result<(), local_chain::UpdateNotConnectedError>,
    ) {
        let mut changeset = LocalChangeSet {
            indexed_additions: {
                let (_, derivation_additions) = self
                    .indexed_graph
                    .index
                    .reveal_to_target_multi(&update.index_update);
                let mut additions = self.indexed_graph.apply_update(update.graph_update);
                additions.index_additions.append(derivation_additions);
                additions
            },
            ..Default::default()
        };

        match self.chain.apply_update(update.chain_update) {
            Ok(chain_changeset) => {
                changeset.chain_changeset = chain_changeset;
                (changeset, Ok(()))
            }
            Err(err) => (changeset, Err(err)),
        }
    }

    pub fn apply_changeset(&mut self, changeset: LocalChangeSet<K, A>) {
        self.indexed_graph
            .apply_additions(changeset.indexed_additions);
        self.chain.apply_changeset(changeset.chain_changeset);
    }

    pub fn insert_block(
        &mut self,
        block_id: BlockId,
    ) -> Result<LocalChangeSet<K, A>, local_chain::InsertBlockNotMatchingError> {
        self.chain
            .insert_block(block_id)
            .map(|chain_changeset| LocalChangeSet {
                chain_changeset,
                ..Default::default()
            })
    }
}

impl<K: Debug + Clone + Ord, A: Anchor, O: ChainOracle> RemoteTracker<K, A, O> {
    pub fn new_remote(
        genesis_blockhash: BlockHash,
        keychains: impl IntoIterator<Item = (K, Descriptor<DescriptorPublicKey>)>,
        oracle: O,
    ) -> Self {
        Self {
            genesis_blockhash,
            indexed_graph: {
                let mut indexed_graph = IndexedTxGraph::<A, KeychainTxOutIndex<K>>::default();
                for (keychain, descriptor) in keychains.into_iter() {
                    indexed_graph.index.add_keychain(keychain, descriptor);
                }
                indexed_graph
            },
            chain: RemoteChain::new(oracle),
        }
    }

    pub fn apply_update(&mut self, update: RemoteUpdate<K, A>) -> RemoteChangeSet<K, A> {
        RemoteChangeSet {
            indexed_additions: {
                let (_, derivation_additions) = self
                    .indexed_graph
                    .index
                    .reveal_to_target_multi(&update.index_update);
                let mut additions = self.indexed_graph.apply_update(update.graph_update);
                additions.index_additions.append(derivation_additions);
                additions
            },
            chain_changeset: match update.chain_update {
                Some(last_seen_block) => self.chain.update_last_seen_block(last_seen_block),
                None => Default::default(),
            },
        }
    }

    pub fn apply_changeset(&mut self, changeset: RemoteChangeSet<K, A>) {
        self.indexed_graph
            .apply_additions(changeset.indexed_additions);
        self.chain.apply_changeset(changeset.chain_changeset);
    }
}

impl<K, A, C> Tracker<K, A, C> {
    pub fn index(&self) -> &KeychainTxOutIndex<K> {
        &self.indexed_graph.index
    }

    pub fn index_mut(&mut self) -> &mut KeychainTxOutIndex<K> {
        &mut self.indexed_graph.index
    }

    pub fn graph(&self) -> &TxGraph<A> {
        self.indexed_graph.graph()
    }

    pub fn chain(&self) -> &C {
        &self.chain
    }

    pub fn genesis_block(&self) -> BlockId {
        BlockId {
            height: 0,
            hash: self.genesis_blockhash,
        }
    }
}

impl<K, A: Anchor, C: ChainOracle + LastSeenBlock> Tracker<K, A, C>
where
    K: Debug + Clone + Ord,
{
    pub fn insert_tx<CC: Default>(
        &mut self,
        tx: &Transaction,
        anchors: impl IntoIterator<Item = A>,
        seen_at: Option<u64>,
    ) -> ChangeSet<K, A, CC> {
        ChangeSet {
            indexed_additions: self.indexed_graph.insert_tx(tx, anchors, seen_at),
            ..Default::default()
        }
    }

    pub fn try_get_transaction(
        &self,
        txid: Txid,
    ) -> Result<Option<CanonicalTx<Transaction, A>>, C::Error> {
        let chain_tip = self.chain.last_seen_block().unwrap_or(self.genesis_block());
        let node = match self.graph().get_tx_node(txid) {
            Some(node) => node,
            None => return Ok(None),
        };
        self.graph()
            .try_get_chain_position(&self.chain, chain_tip, txid)
            .map(|opt| opt.map(|observed_as| CanonicalTx { observed_as, node }))
    }

    pub fn try_list_transactions(
        &self,
    ) -> impl Iterator<Item = Result<CanonicalTx<Transaction, A>, C::Error>> {
        let chain_tip = self.chain.last_seen_block().unwrap_or(self.genesis_block());
        self.indexed_graph
            .graph()
            .try_list_chain_txs(&self.chain, chain_tip)
    }

    pub fn try_list_owned_txouts(
        &self,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), C::Error>> {
        let chain_tip = self.chain.last_seen_block().unwrap_or(self.genesis_block());
        self.indexed_graph
            .graph()
            .try_list_chain_txouts(&self.chain, chain_tip)
            .filter_map(|r| match r {
                Err(err) => Some(Err(err)),
                Ok(full_txo) => Some(Ok((
                    self.indexed_graph
                        .index
                        .index_of_spk(&full_txo.txout.script_pubkey)?,
                    full_txo,
                ))),
            })
    }

    pub fn try_list_owned_unspents(
        &self,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), C::Error>> {
        self.try_list_owned_txouts().filter(|r| {
            if let Ok((_, full_txo)) = r {
                if full_txo.spent_by.is_some() {
                    return false;
                }
            }
            true
        })
    }

    pub fn try_balance(&self, should_trust: impl Fn(&K) -> bool) -> Result<Balance, C::Error> {
        let chain_tip = self.chain.last_seen_block().unwrap_or(self.genesis_block());
        self.indexed_graph
            .try_balance(&self.chain, chain_tip, |script| {
                match self.index().index_of_spk(script) {
                    Some((keychain, _)) => should_trust(keychain),
                    None => false,
                }
            })
    }
}

impl<K, A: Anchor, C: ChainOracle<Error = Infallible> + LastSeenBlock> Tracker<K, A, C>
where
    K: Debug + Clone + Ord,
{
    pub fn get_transaction(&self, txid: Txid) -> Option<CanonicalTx<Transaction, A>> {
        self.try_get_transaction(txid)
            .expect("oracle is infallible")
    }

    pub fn list_transactions(&self) -> impl Iterator<Item = CanonicalTx<Transaction, A>> {
        self.try_list_transactions()
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn list_owned_txouts(&self) -> impl Iterator<Item = (&(K, u32), FullTxOut<ObservedAs<A>>)> {
        self.try_list_owned_txouts()
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn list_owned_unspents(
        &self,
    ) -> impl Iterator<Item = (&(K, u32), FullTxOut<ObservedAs<A>>)> {
        self.try_list_owned_unspents()
            .map(|r| r.expect("oracle is infallible"))
    }

    pub fn balance(&self, should_trust: impl Fn(&K) -> bool) -> Balance {
        self.try_balance(should_trust)
            .expect("oracle is infallible")
    }
}

#[derive(Debug, PartialEq)]
pub struct Update<K, A, CC> {
    pub index_update: BTreeMap<K, u32>,
    pub graph_update: TxGraph<A>,
    pub chain_update: CC,
}

/// A structure containing the resultant changes to [`Tracker`] after an update.
#[derive(Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(
        crate = "serde_crate",
        bound(
            deserialize = "A: Ord + serde::Deserialize<'de>, K: Ord + serde::Deserialize<'de>, CC: Ord + serde::Deserialize<'de>",
            serialize = "A: Ord + serde::Serialize, K: Ord + serde::Serialize, CC: Ord + serde::Serialize",
        )
    )
)]
#[must_use]
pub struct ChangeSet<K, A, CC> {
    pub indexed_additions: IndexedAdditions<A, DerivationAdditions<K>>,
    pub chain_changeset: CC,
}

impl<K, A, C: Default> Default for ChangeSet<K, A, C> {
    fn default() -> Self {
        Self {
            indexed_additions: Default::default(),
            chain_changeset: Default::default(),
        }
    }
}

impl<K: Ord, A: Anchor, CC: Append> Append for ChangeSet<K, A, CC> {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.indexed_additions, other.indexed_additions);
        Append::append(&mut self.chain_changeset, other.chain_changeset)
    }
}

impl<K, A> LocalChangeSet<K, A> {
    pub fn is_empty(&self) -> bool {
        self.indexed_additions.index_additions.is_empty()
            && self.indexed_additions.graph_additions.is_empty()
            && self.chain_changeset.is_empty()
    }
}

impl<K, A> RemoteChangeSet<K, A> {
    pub fn is_empty(&self) -> bool {
        self.indexed_additions.index_additions.is_empty()
            && self.indexed_additions.graph_additions.is_empty()
            && self.chain_changeset.is_none()
    }
}
