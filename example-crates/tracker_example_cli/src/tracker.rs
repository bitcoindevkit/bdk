use std::fmt::Debug;

use bdk_chain::{
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{DerivationAdditions, KeychainTxOutIndex},
    Anchor, Append, BlockId, ChainOracle, FullTxOut, LoadablePersistBackend, ObservedAs,
};
use bdk_file_store::{IterError, Store};

pub type TrackerStore<A, K> = Store<Tracker<A, K>, ChangeSet<A, K>>;

#[derive(Default)]
pub struct Tracker<A, K> {
    pub inner: IndexedTxGraph<A, KeychainTxOutIndex<K>>,
    last_seen_height: Option<u32>,
}

impl<A: Default, K: Default> Tracker<A, K> {
    pub fn last_seen_height(&self) -> Option<u32> {
        self.last_seen_height
    }

    pub fn update_last_seen_height(&mut self, last_seen_height: Option<u32>) -> ChangeSet<A, K> {
        if self.last_seen_height < last_seen_height {
            self.last_seen_height = Ord::max(self.last_seen_height, last_seen_height);
            ChangeSet {
                last_seen_height,
                ..Default::default()
            }
        } else {
            Default::default()
        }
    }
}

impl<A: Anchor, K: Clone + Ord + Debug> Tracker<A, K> {
    pub fn apply_changeset(&mut self, changeset: ChangeSet<A, K>) {
        self.inner.apply_additions(changeset.additions);
        self.last_seen_height = Ord::max(self.last_seen_height, changeset.last_seen_height);
    }

    pub fn try_list_owned_txouts<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), C::Error>> + 'a {
        self.inner
            .graph()
            .try_list_chain_txouts(chain, chain_tip)
            .filter_map(|r| match r {
                Err(err) => Some(Err(err)),
                Ok(full_txo) => Some(Ok((
                    self.inner
                        .index
                        .index_of_spk(&full_txo.txout.script_pubkey)?,
                    full_txo,
                ))),
            })
    }

    pub fn try_list_owned_unspents<'a, C: ChainOracle + 'a>(
        &'a self,
        chain: &'a C,
        chain_tip: BlockId,
    ) -> impl Iterator<Item = Result<(&(K, u32), FullTxOut<ObservedAs<A>>), C::Error>> + 'a {
        self.try_list_owned_txouts(chain, chain_tip).filter(|r| {
            if let Ok((_, full_txo)) = r {
                if full_txo.spent_by.is_some() {
                    return false;
                }
            }
            true
        })
    }
}

#[derive(Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(bound(
    deserialize = "A: Ord + serde::Deserialize<'de>, K: Ord + serde::Deserialize<'de>",
    serialize = "A: Ord + serde::Serialize, K: Ord + serde::Serialize",
))]
pub struct ChangeSet<A, K> {
    pub additions: IndexedAdditions<A, DerivationAdditions<K>>,
    pub last_seen_height: Option<u32>,
}

impl<A: Anchor, K: Ord> Append for ChangeSet<A, K> {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.additions, other.additions);
        self.last_seen_height = Ord::max(self.last_seen_height, other.last_seen_height);
    }
}

impl<A: Anchor, K> From<IndexedAdditions<A, DerivationAdditions<K>>> for ChangeSet<A, K> {
    fn from(additions: IndexedAdditions<A, DerivationAdditions<K>>) -> Self {
        let last_seen_height = additions
            .graph_additions
            .anchors
            .iter()
            .last()
            .map(|(a, _)| a.confirmation_height_upper_bound());
        ChangeSet {
            additions,
            last_seen_height,
        }
    }
}

impl<A, K> From<DerivationAdditions<K>> for ChangeSet<A, K> {
    fn from(index_additions: DerivationAdditions<K>) -> Self {
        ChangeSet {
            additions: IndexedAdditions {
                graph_additions: Default::default(),
                index_additions,
            },
            last_seen_height: None,
        }
    }
}

impl<A, K> LoadablePersistBackend<Tracker<A, K>, ChangeSet<A, K>> for TrackerStore<A, K>
where
    A: Anchor + Default + serde::de::DeserializeOwned + serde::Serialize,
    K: Default + Ord + Clone + Debug + serde::de::DeserializeOwned + serde::Serialize,
{
    type LoadError = IterError;

    fn load_into_tracker(&mut self, tracker: &mut Tracker<A, K>) -> Result<(), Self::LoadError> {
        let (changeset, res) = self.aggregate_changesets();
        tracker.apply_changeset(changeset);
        res
    }
}
