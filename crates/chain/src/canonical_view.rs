use core::{convert::Infallible, fmt::Debug};

use crate::{
    alloc::{collections::VecDeque, vec::Vec},
    collections::{HashMap, HashSet},
    tx_graph::{TxDescendants, TxNode},
    Anchor, Balance, ChainOracle, TxGraph, UnconfirmedOracle, COINBASE_MATURITY,
};
use alloc::sync::Arc;
use bdk_core::BlockId;
use bitcoin::{Amount, OutPoint, Script, Transaction, TxOut, Txid};

/// A transaction that is part of the [`CanonicalView`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalTx<A, U> {
    /// The position of the transaction in the [`CanonicalView`].
    pub pos: CanonicalPos<A, U>,
    /// The txid.
    pub txid: Txid,
    /// The transaction.
    pub tx: Arc<Transaction>,
}

/// A transaction output that is part of the [`CanonicalView`].
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct CanonicalTxOut<A, U, I> {
    /// The position of the txout's residing transaction in the [`CanonicalView`].
    pub pos: CanonicalPos<A, U>,
    /// The index of the script pubkey of this txout.
    pub spk_index: I,
    /// The outpoint.
    pub outpoint: OutPoint,
    /// The txout.
    pub txout: TxOut,
    /// The canonical transaction spending this txout (if any).
    pub spent_by: Option<Txid>,
    /// Whether this output resides in a coinbase transaction.
    pub is_on_coinbase: bool,
}

impl<A, U, I> CanonicalTxOut<A, U, I> {
    /// Whether txout belongs in a confirmed transaction.
    pub fn is_confirmed(&self) -> bool {
        match self.pos {
            CanonicalPos::Confirmed(_) => true,
            CanonicalPos::Unconfirmed(_) => false,
        }
    }
}

impl<A: Anchor, U, I> CanonicalTxOut<A, U, I> {
    /// Whether the `txout` is considered mature.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_mature(&self, tip_height: u32) -> bool {
        if self.is_on_coinbase {
            let tx_height = match &self.pos {
                CanonicalPos::Confirmed(anchor) => anchor.confirmation_height_upper_bound(),
                CanonicalPos::Unconfirmed(_) => {
                    debug_assert!(false, "coinbase tx can never be unconfirmed");
                    return false;
                }
            };
            let age = tip_height.saturating_sub(tx_height);
            if age + 1 < COINBASE_MATURITY {
                return false;
            }
        }
        true
    }
}

/// A consistent view of transactions.
///
/// Function:
/// * Return ordered history of transactions.
/// * Quickly query whether a txid is in the canonical history and it's position.
#[derive(Debug, Clone)]
pub struct CanonicalView<A, U> {
    pub(crate) tip: BlockId,
    pub(crate) txs: HashMap<Txid, (CanonicalPos<A, U>, Arc<Transaction>)>,
    pub(crate) ordered_txids: Vec<Txid>,
    pub(crate) spends: HashMap<OutPoint, Txid>,
}

/// The position of the transaction in a [`CanonicalView`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: serde::Deserialize<'de>, U: serde::Deserialize<'de>",
        serialize = "A: serde::Serialize, U: serde::Serialize"
    ))
)]
pub enum CanonicalPos<A, U> {
    /// Confirmed with anchor `A`.
    Confirmed(A),
    /// Unconfirmed, positioned with `U`.
    Unconfirmed(U),
}

impl<A, U> CanonicalPos<A, U> {
    /// Whether the [`CanonicalPos`] is a confirmed position.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }

    /// Get the upper bound of the chain data's confirmation height.
    ///
    /// Refer to [`Anchor::confirmation_height_upper_bound`].
    pub fn confirmation_height_upper_bound(&self) -> Option<u32>
    where
        A: Anchor,
    {
        match self {
            CanonicalPos::Confirmed(anchor) => Some(anchor.confirmation_height_upper_bound()),
            CanonicalPos::Unconfirmed(_) => None,
        }
    }
}

impl<A: Anchor, U: Ord + Clone> CanonicalView<A, U> {
    /// Yoo
    pub fn new<CO: ChainOracle, UO: UnconfirmedOracle<UnconfirmedPos = U>>(
        chain_oracle: &CO,
        chain_tip: BlockId,
        unconf_oracle: &UO,
        tx_graph: &TxGraph<A>,
    ) -> Result<Self, CanonicalError<CO::Error, UO::Error>> {
        // Each outpoint represents a set of conflicting edges.
        let mut next_ops = VecDeque::<OutPoint>::new();
        let mut visited_ops = HashSet::<OutPoint>::new();
        let mut included = HashMap::<Txid, (CanonicalPos<A, U>, Arc<Transaction>)>::new();
        let mut included_in_order = Vec::<Txid>::new();
        let mut excluded = HashSet::<Txid>::new();

        fn insert_canon_tx<A, U>(
            tx_node: &TxNode<Arc<Transaction>, A>,
            canonical_pos: CanonicalPos<A, U>,
            included: &mut HashMap<Txid, (CanonicalPos<A, U>, Arc<Transaction>)>,
            included_in_order: &mut Vec<Txid>,
            next_ops: &mut VecDeque<OutPoint>,
        ) -> bool {
            let is_new = included
                .insert(tx_node.txid, (canonical_pos, tx_node.tx.clone()))
                .is_none();
            if is_new {
                included_in_order.push(tx_node.txid);
                next_ops.extend(
                    (0..tx_node.output.len() as u32).map(|vout| OutPoint::new(tx_node.txid, vout)),
                );
            }
            is_new
        }

        for tx_node in tx_graph.coinbase_txs() {
            for anchor in tx_node.anchors {
                let is_canon = chain_oracle
                    .is_block_in_chain(anchor.anchor_block(), chain_tip)
                    .map_err(CanonicalError::ChainOracle)?;
                if is_canon == Some(true)
                    && insert_canon_tx(
                        &tx_node,
                        CanonicalPos::Confirmed(anchor.clone()),
                        &mut included,
                        &mut included_in_order,
                        &mut next_ops,
                    )
                {
                    break;
                }
            }
        }
        next_ops.extend(tx_graph.root_outpoints());

        //println!("[START: CREATE CANONICAL VIEW]");
        while let Some(op) = next_ops.pop_front() {
            if !visited_ops.insert(op) || excluded.contains(&op.txid) {
                continue;
            }
            //println!("ITERATION: {}", op);

            let conflicts = tx_graph
                .outspends(op)
                .iter()
                .copied()
                .filter(|txid| !excluded.contains(txid))
                .collect::<Vec<Txid>>();

            let mut conflicts_iter = conflicts.iter();
            let mut canon_txid: Option<Txid> = 'find_canon_txid: loop {
                let txid = match conflicts_iter.next() {
                    Some(&txid) => txid,
                    None => break None,
                };
                if let Some((CanonicalPos::Confirmed(_), _)) = included.get(&txid) {
                    break Some(txid);
                }
                if let Some(tx_node) = tx_graph.get_tx_node(txid) {
                    for anchor in tx_node.anchors {
                        let is_canon = chain_oracle
                            .is_block_in_chain(anchor.anchor_block(), chain_tip)
                            .map_err(CanonicalError::ChainOracle)?;
                        if is_canon == Some(true) {
                            assert!(
                                insert_canon_tx(
                                    &tx_node,
                                    CanonicalPos::Confirmed(anchor.clone()),
                                    &mut included,
                                    &mut included_in_order,
                                    &mut next_ops,
                                ),
                                "we just checked that tx is not already canonical with anchor"
                            );
                            break 'find_canon_txid Some(txid);
                        }
                    }
                }
            };
            if canon_txid.is_none() {
                if let Some((unconf_pos, txid)) = unconf_oracle
                    .pick_canonical(tx_graph, conflicts.clone())
                    .map_err(CanonicalError::UnconfirmedOracle)?
                {
                    if let Some(tx_node) = tx_graph.get_tx_node(txid) {
                        insert_canon_tx(
                            &tx_node,
                            CanonicalPos::Unconfirmed(unconf_pos),
                            &mut included,
                            &mut included_in_order,
                            &mut next_ops,
                        );
                        //println!("\t FOUND UNCONFIRMED CANON: {}", txid);
                        canon_txid = Some(txid);
                    };
                }
            }

            let _n = TxDescendants::from_multiple_include_root(
                tx_graph,
                conflicts
                    .into_iter()
                    .filter(|&txid| Some(txid) != canon_txid),
                |_: usize, txid| {
                    if excluded.insert(txid) {
                        //println!("\t EXCLUDED: {}", txid);
                        Some(())
                    } else {
                        None
                    }
                },
            )
            .count();
        }

        // remove excluded elements and sort.
        included_in_order.retain(|txid| !excluded.contains(txid));
        included.retain(|txid, _| !excluded.contains(txid));
        let spends = included
            .iter()
            .flat_map(|(txid, (_, tx))| tx.input.iter().map(|txin| (txin.previous_output, *txid)))
            .collect();
        Ok(Self {
            tip: chain_tip,
            txs: included,
            ordered_txids: included_in_order,
            spends,
        })
    }

    /// Whether this view has no transactions.
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Returns the number of transactions in the view.
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    /// Get the canonical transaction of txid.
    pub fn tx(&self, txid: Txid) -> Option<CanonicalTx<A, U>> {
        get_tx(&self.txs, txid)
    }

    /// Get the canonical output of the given outpoint.
    pub fn txout<I>(
        &self,
        outpoint: OutPoint,
        index_from_outpoint: impl Fn(OutPoint) -> Option<I>,
    ) -> Option<CanonicalTxOut<A, U, I>> {
        let spk_i = index_from_outpoint(outpoint)?;
        self.filter_txouts(core::iter::once((spk_i, outpoint)))
            .next()
    }

    /// Get the canonical unspent output of the given outpoint.
    pub fn unspent<I>(
        &self,
        outpoint: OutPoint,
        index_from_outpoint: impl Fn(OutPoint) -> Option<I>,
    ) -> Option<CanonicalTxOut<A, U, I>> {
        let spk_i = index_from_outpoint(outpoint)?;
        self.filter_unspents(core::iter::once((spk_i, outpoint)))
            .next()
    }

    /// Get spend for given output (if any).
    pub fn spend(&self, outpoint: OutPoint) -> Option<Txid> {
        self.spends.get(&outpoint).copied()
    }

    /// Get spend for given output (if any), alongside the spending tx's canoncial position.
    pub fn spend_with_pos(&self, outpoint: OutPoint) -> Option<(CanonicalPos<A, U>, Txid)> {
        let spend_txid = self.spends.get(&outpoint).copied()?;
        let spend_pos = self.tx(spend_txid).expect("must exist").pos;
        Some((spend_pos, spend_txid))
    }

    /// Sort the canonical txs by key.
    pub fn sort_txs_by_key<K, F>(&mut self, mut f: F)
    where
        F: FnMut(&Txid, &CanonicalPos<A, U>) -> K,
        K: Ord,
    {
        let txs = &self.txs;
        self.ordered_txids.sort_by_key(move |txid| {
            let (pos, _) = txs.get(txid).expect("must have corresponding tx");
            f(txid, pos)
        });
    }

    /// Iterate over all transactions of the [`CanonicalView`].
    pub fn txs(
        &self,
    ) -> impl ExactSizeIterator<Item = CanonicalTx<A, U>> + DoubleEndedIterator + '_ {
        self.ordered_txids
            .iter()
            .map(|&txid| get_tx(&self.txs, txid).expect("corresponding tx must exist"))
    }

    /// Iterate over all transactions of [`CanonicalView`] by owning it.
    pub fn into_txs(
        self,
    ) -> impl ExactSizeIterator<Item = CanonicalTx<A, U>> + DoubleEndedIterator {
        self.ordered_txids
            .into_iter()
            .map(move |txid| get_tx(&self.txs, txid).expect("corresponding tx must exist"))
    }

    /// Obtain a subset of `outpoints` that are canonical.
    pub fn filter_txouts<'a, I: 'a>(
        &'a self,
        outpoints: impl IntoIterator<Item = (I, OutPoint)> + 'a,
    ) -> impl Iterator<Item = CanonicalTxOut<A, U, I>> + 'a {
        outpoints.into_iter().filter_map(|(spk_index, outpoint)| {
            get_txout(&self.txs, &self.spends, spk_index, outpoint)
        })
    }

    /// Obtain a subset of `outpoints` that are canonical by taking ownership.
    pub fn into_filter_txouts<I>(
        self,
        outpoints: impl IntoIterator<Item = (I, OutPoint)>,
    ) -> impl Iterator<Item = CanonicalTxOut<A, U, I>> {
        outpoints
            .into_iter()
            .filter_map(move |(spk_index, outpoint)| {
                get_txout(&self.txs, &self.spends, spk_index, outpoint)
            })
    }

    /// Obtain a subset of `outpoints` that are canonical and unspent.
    pub fn filter_unspents<'a, I: 'a>(
        &'a self,
        outpoints: impl IntoIterator<Item = (I, OutPoint)> + 'a,
    ) -> impl Iterator<Item = CanonicalTxOut<A, U, I>> + 'a {
        self.filter_txouts(outpoints)
            .filter(|txo| txo.spent_by.is_none())
    }

    /// Obtain a subset of `outpoints` that are canonical and unspent by taking ownership.
    pub fn into_filter_unspents<I>(
        self,
        outpoints: impl IntoIterator<Item = (I, OutPoint)>,
    ) -> impl Iterator<Item = CanonicalTxOut<A, U, I>> {
        self.into_filter_txouts(outpoints)
            .filter(|txo| txo.spent_by.is_none())
    }

    /// Get the total balance of `outpoints`.
    pub fn balance<I>(
        &self,
        outpoints: impl IntoIterator<Item = (I, OutPoint)>,
        mut trust_predicate: impl FnMut(&I, &Script) -> bool,
    ) -> Balance {
        let mut immature = Amount::ZERO;
        let mut trusted_pending = Amount::ZERO;
        let mut untrusted_pending = Amount::ZERO;
        let mut confirmed = Amount::ZERO;

        for txout in self.filter_unspents(outpoints) {
            match &txout.pos {
                CanonicalPos::Confirmed(_) => {
                    if txout.is_mature(self.tip.height) {
                        confirmed += txout.txout.value;
                    } else {
                        immature += txout.txout.value;
                    }
                }
                CanonicalPos::Unconfirmed(_) => {
                    if trust_predicate(&txout.spk_index, txout.txout.script_pubkey.as_script()) {
                        trusted_pending += txout.txout.value;
                    } else {
                        untrusted_pending += txout.txout.value;
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
}

fn get_tx<A: Anchor, U: Ord + Clone>(
    txs: &HashMap<Txid, (CanonicalPos<A, U>, Arc<Transaction>)>,
    txid: Txid,
) -> Option<CanonicalTx<A, U>> {
    txs.get(&txid)
        .cloned()
        .map(|(pos, tx)| CanonicalTx { pos, txid, tx })
}

fn get_txout<A: Anchor, U: Ord + Clone, I>(
    txs: &HashMap<Txid, (CanonicalPos<A, U>, Arc<Transaction>)>,
    spends: &HashMap<OutPoint, Txid>,
    spk_index: I,
    outpoint: OutPoint,
) -> Option<CanonicalTxOut<A, U, I>> {
    let (pos, tx) = txs.get(&outpoint.txid).cloned()?;
    let txout = tx.output.get(outpoint.vout as usize).cloned()?;
    let spent_by = spends.get(&outpoint).copied();
    let is_on_coinbase = tx.is_coinbase();
    Some(CanonicalTxOut {
        pos,
        spk_index,
        outpoint,
        txout,
        spent_by,
        is_on_coinbase,
    })
}

/// Occurs when constructing a [`CanonicalView`] fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalError<CE, UE> {
    /// The [`ChainOracle`] impl errored.
    ChainOracle(CE),
    /// The [`UnconfirmedOracle`] impl errored.
    UnconfirmedOracle(UE),
}

impl<UE> CanonicalError<Infallible, UE> {
    /// Transform into an [`UnconfirmedOracle`] error.
    pub fn into_unconfirmed_oracle_error(self) -> UE {
        match self {
            CanonicalError::ChainOracle(_) => unreachable!("error is infallible"),
            CanonicalError::UnconfirmedOracle(err) => err,
        }
    }
}

impl<CE> CanonicalError<CE, Infallible> {
    /// Transform into a [`ChainOracle`] error.
    pub fn into_chain_oracle_error(self) -> CE {
        match self {
            CanonicalError::ChainOracle(err) => err,
            CanonicalError::UnconfirmedOracle(_) => unreachable!("error is infallible"),
        }
    }
}

impl<CE: core::fmt::Display, UE: core::fmt::Display> core::fmt::Display for CanonicalError<CE, UE> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CanonicalError::ChainOracle(err) => write!(
                f,
                "chain oracle errored while constructing canonical view: {}",
                err
            ),
            CanonicalError::UnconfirmedOracle(err) => write!(
                f,
                "unconfirmed oracle errored while constructing canonical view: {}",
                err
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<CE: std::error::Error, UE: std::error::Error> std::error::Error for CanonicalError<CE, UE> {}
