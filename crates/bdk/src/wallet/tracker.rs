use alloc::vec::Vec;
use bdk_chain::{
    indexed_tx_graph::{IndexedAdditions, IndexedTxGraph},
    keychain::{Balance, DerivationAdditions, KeychainTxOutIndex},
    local_chain::{self, LocalChain},
    tx_graph::{CanonicalTx, TxGraph},
    Append, BlockId, ConfirmationTimeAnchor, ObservedAs,
};
use bitcoin::{Transaction, Txid};

use crate::KeychainKind;

pub type FullTxOut = bdk_chain::FullTxOut<ObservedAs<ConfirmationTimeAnchor>>;

#[derive(Debug, Default)]
pub struct Tracker {
    pub indexed_graph: IndexedTxGraph<ConfirmationTimeAnchor, KeychainTxOutIndex<KeychainKind>>,
    pub chain: LocalChain,
}

impl Tracker {
    pub fn chain(&self) -> &LocalChain {
        &self.chain
    }

    pub fn graph(&self) -> &TxGraph<ConfirmationTimeAnchor> {
        self.indexed_graph.graph()
    }

    pub fn index(&self) -> &KeychainTxOutIndex<KeychainKind> {
        &self.indexed_graph.index
    }

    pub fn index_mut(&mut self) -> &mut KeychainTxOutIndex<KeychainKind> {
        &mut self.indexed_graph.index
    }

    pub fn insert_tx(
        &mut self,
        tx: Transaction,
        anchors: impl IntoIterator<Item = ConfirmationTimeAnchor>,
        seen_at: Option<u64>,
    ) -> Result<ChangeSet, InsertTxInvalidAnchorError> {
        let txid = tx.txid();

        let anchors = anchors
            .into_iter()
            .map(|a| {
                if a.anchor_block.height < a.confirmation_height {
                    Err(InsertTxInvalidAnchorError {
                        txid,
                        tx_height: a.confirmation_height,
                        anchor_block_height: a.anchor_block.height,
                    })
                } else {
                    Ok(a)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(self.indexed_graph.insert_tx(&tx, anchors, seen_at).into())
    }

    pub fn insert_block(
        &mut self,
        block_id: BlockId,
    ) -> Result<ChangeSet, local_chain::InsertBlockNotMatchingError> {
        self.chain.insert_block(block_id).map(Into::into)
    }

    pub fn apply_changeset(&mut self, changeset: ChangeSet) {
        self.indexed_graph
            .apply_additions(changeset.indexed_additions);
        self.chain.apply_changeset(changeset.chain_changeset);
    }

    pub fn list_owned_txouts(&self) -> impl Iterator<Item = (&(KeychainKind, u32), FullTxOut)> {
        // [TODO] Use block id of correct genesis block
        let chain_tip = self.chain.tip().unwrap_or_default();

        self.indexed_graph
            .graph()
            .list_chain_txouts(&self.chain, chain_tip)
            .filter_map(|full_txo| {
                let keychain_ind = self.index().index_of_spk(&full_txo.txout.script_pubkey)?;
                Some((keychain_ind, full_txo))
            })
    }

    pub fn list_owned_unspents(&self) -> impl Iterator<Item = (&(KeychainKind, u32), FullTxOut)> {
        self.list_owned_txouts()
            .filter(|(_, full_txo)| full_txo.spent_by.is_none())
    }

    pub fn list_transactions(
        &self,
    ) -> impl Iterator<Item = CanonicalTx<Transaction, ConfirmationTimeAnchor>> {
        // [TODO] Use block id of correct genesis block
        let chain_tip = self.chain.tip().unwrap_or_default();
        self.graph().list_chain_txs(&self.chain, chain_tip)
    }

    pub fn balance(&self) -> Balance {
        let chain_tip = self.chain.tip().unwrap_or_default();
        self.indexed_graph.balance(&self.chain, chain_tip, |spk| {
            matches!(
                self.indexed_graph.index.index_of_spk(spk),
                Some(&(KeychainKind::Internal, _))
            )
        })
    }

    pub fn get_tx(&self, txid: Txid) -> Option<CanonicalTx<Transaction, ConfirmationTimeAnchor>> {
        let chain_tip = self.chain().tip().unwrap_or_default();

        let node = self.graph().get_tx_node(txid)?;
        let observed_as = self
            .graph()
            .get_chain_position(self.chain(), chain_tip, txid)?;
        Some(CanonicalTx { observed_as, node })
    }
}

/// The changeset produced internally by applying an update
#[derive(Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ChangeSet {
    pub indexed_additions:
        IndexedAdditions<ConfirmationTimeAnchor, DerivationAdditions<KeychainKind>>,
    pub chain_changeset: local_chain::ChangeSet,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.indexed_additions.graph_additions.is_empty()
            && self.indexed_additions.index_additions.is_empty()
            && self.chain_changeset.is_empty()
    }
}

impl Append for ChangeSet {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.indexed_additions, other.indexed_additions);
        Append::append(&mut self.chain_changeset, other.chain_changeset);
    }
}

impl From<DerivationAdditions<KeychainKind>> for ChangeSet {
    fn from(index_additions: DerivationAdditions<KeychainKind>) -> Self {
        Self {
            indexed_additions: IndexedAdditions {
                index_additions,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

impl From<local_chain::ChangeSet> for ChangeSet {
    fn from(chain_changeset: local_chain::ChangeSet) -> Self {
        Self {
            chain_changeset,
            ..Default::default()
        }
    }
}

impl From<IndexedAdditions<ConfirmationTimeAnchor, DerivationAdditions<KeychainKind>>>
    for ChangeSet
{
    fn from(
        indexed_additions: IndexedAdditions<
            ConfirmationTimeAnchor,
            DerivationAdditions<KeychainKind>,
        >,
    ) -> Self {
        Self {
            indexed_additions,
            chain_changeset: Default::default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InsertTxInvalidAnchorError {
    pub txid: Txid,
    pub tx_height: u32,
    pub anchor_block_height: u32,
}

impl std::fmt::Display for InsertTxInvalidAnchorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cannot insert tx ({}) with anchor block height ({}) higher than tx confirmation height ({})", self.txid, self.anchor_block_height, self.tx_height)
    }
}

impl std::error::Error for InsertTxInvalidAnchorError {}
