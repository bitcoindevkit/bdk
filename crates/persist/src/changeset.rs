#![cfg(feature = "miniscript")]

use bdk_chain::{bitcoin::Network, indexed_tx_graph, keychain, local_chain, Anchor, Append};

/// Changes from a combination of [`bdk_chain`] structures.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(bdk_chain::serde::Deserialize, bdk_chain::serde::Serialize),
    serde(
        crate = "bdk_chain::serde",
        bound(
            deserialize = "A: Ord + bdk_chain::serde::Deserialize<'de>, K: Ord + bdk_chain::serde::Deserialize<'de>",
            serialize = "A: Ord + bdk_chain::serde::Serialize, K: Ord + bdk_chain::serde::Serialize",
        ),
    )
)]
pub struct CombinedChangeSet<K, A> {
    /// Changes to the [`LocalChain`](local_chain::LocalChain).
    pub chain: local_chain::ChangeSet,
    /// Changes to [`IndexedTxGraph`](indexed_tx_graph::IndexedTxGraph).
    pub indexed_tx_graph: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>,
    /// Stores the network type of the transaction data.
    pub network: Option<Network>,
}

impl<K, A> Default for CombinedChangeSet<K, A> {
    fn default() -> Self {
        Self {
            chain: Default::default(),
            indexed_tx_graph: Default::default(),
            network: None,
        }
    }
}

impl<K: Ord, A: Anchor> Append for CombinedChangeSet<K, A> {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.chain, other.chain);
        Append::append(&mut self.indexed_tx_graph, other.indexed_tx_graph);
        if other.network.is_some() {
            debug_assert!(
                self.network.is_none() || self.network == other.network,
                "network type must either be just introduced or remain the same"
            );
            self.network = other.network;
        }
    }

    fn is_empty(&self) -> bool {
        self.chain.is_empty() && self.indexed_tx_graph.is_empty() && self.network.is_none()
    }
}

impl<K, A> From<local_chain::ChangeSet> for CombinedChangeSet<K, A> {
    fn from(chain: local_chain::ChangeSet) -> Self {
        Self {
            chain,
            ..Default::default()
        }
    }
}

impl<K, A> From<indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>>
    for CombinedChangeSet<K, A>
{
    fn from(indexed_tx_graph: indexed_tx_graph::ChangeSet<A, keychain::ChangeSet<K>>) -> Self {
        Self {
            indexed_tx_graph,
            ..Default::default()
        }
    }
}
