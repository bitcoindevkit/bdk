//! Module for structures that maintain sparse (purposely incomplete) snapshots of blockchain data.
//!
//! [`SparseChain`] stores [`Txid`]s ordered by an index that implements [`ChainPosition`] (this
//! represents the transaction's position in the blockchain, by default [`TxHeight`] is used).
//! [`SparseChain`] also contains "checkpoints" which relate block height to block hash. Changes to
//! a [`SparseChain`] are reported by returning [`ChangeSet`]s.
//!
//! # Updating [`SparseChain`]
//!
//! A sparsechain can be thought of as a consistent snapshot of history. A [`SparseChain`] can be
//! updated by applying an update [`SparseChain`] on top, but only if they "connect" via their
//! checkpoints and don't result in unexpected movements of transactions.
//!
//! ```
//! # use bdk_chain::{BlockId, TxHeight, sparse_chain::*, example_utils::*};
//! # use bitcoin::BlockHash;
//! # let hash_a = new_hash::<BlockHash>("a");
//! # let hash_b = new_hash::<BlockHash>("b");
//! # let hash_c = new_hash::<BlockHash>("c");
//! # let hash_d = new_hash::<BlockHash>("d");
//! // create empty sparsechain
//! let mut chain = SparseChain::<TxHeight>::default();
//!
//! /* Updating an empty sparsechain will always succeed */
//!
//! let update = SparseChain::from_checkpoints(vec![
//!     BlockId { height: 1, hash: hash_a },
//!     BlockId { height: 2, hash: hash_b },
//! ]);
//! let _ = chain
//!     .apply_update(update)
//!     .expect("updating an empty sparsechain will always succeed");
//!
//! /* To update a non-empty sparsechain, the update must connect */
//!
//! let update = SparseChain::from_checkpoints(vec![
//!     BlockId { height: 2, hash: hash_b },
//!     BlockId { height: 3, hash: hash_c },
//! ]);
//! let _ = chain
//!     .apply_update(update)
//!     .expect("we have connected at block height 2, so this must succeed");
//! ```
//!
//! ## Invalid updates
//!
//! As shown above, sparsechains can be "connected" by comparing their checkpoints. However, there
//! are situations where two sparsechains cannot connect in a way that guarantees consistency.
//!
//! ```
//! # use bdk_chain::{BlockId, TxHeight, sparse_chain::*, example_utils::*};
//! # use bitcoin::BlockHash;
//! # let hash_a = new_hash::<BlockHash>("a");
//! # let hash_b = new_hash::<BlockHash>("b");
//! # let hash_c = new_hash::<BlockHash>("c");
//! # let hash_d = new_hash::<BlockHash>("d");
//! // our sparsechain has 2 checkpoints
//! let chain = SparseChain::<TxHeight>::from_checkpoints(vec![
//!     BlockId { height: 1, hash: hash_a },
//!     BlockId { height: 2, hash: hash_b },
//! ]);
//!
//! /* Example of an ambiguous update that does not fully connect */
//!
//! let ambiguous_update = SparseChain::from_checkpoints(vec![
//!     // the update sort of "connects" at checkpoint 1, but...
//!     BlockId { height: 1, hash: hash_a },
//!     // we cannot determine whether checkpoint 3 connects with checkpoint 2
//!     BlockId { height: 3, hash: hash_c },
//! ]);
//! let _ = chain
//!     .determine_changeset(&ambiguous_update)
//!     .expect_err("cannot apply ambiguous update");
//!
//! /* Example of an update that completely misses the point */
//!
//! let disconnected_update = SparseChain::from_checkpoints(vec![
//!     // the last checkpoint in chain is 2, so 3 and 4 do not connect
//!     BlockId { height: 3, hash: hash_c },
//!     BlockId { height: 4, hash: hash_d },
//! ]);
//! let _ = chain
//!     .determine_changeset(&disconnected_update)
//!     .expect_err("cannot apply a totally-disconnected update");
//! ```
//!
//! ## Handling reorgs
//!
//! Updates can be formed to evict data from the original sparsechain. This is useful for handling
//! blockchain reorgs.
//!
//! ```
//! # use bdk_chain::{BlockId, TxHeight, sparse_chain::*, example_utils::*};
//! # use bitcoin::BlockHash;
//! # let hash_a = new_hash::<BlockHash>("a");
//! # let hash_b = new_hash::<BlockHash>("b");
//! # let hash_c = new_hash::<BlockHash>("c");
//! # let hash_d = new_hash::<BlockHash>("d");
//! // our chain has a single checkpoint at height 11
//! let mut chain = SparseChain::<TxHeight>::from_checkpoints(vec![
//!     BlockId { height: 11, hash: hash_a },
//! ]);
//!
//! // we detect a reorg at height 11, and we introduce a new checkpoint at height 12
//! let update = SparseChain::from_checkpoints(vec![
//!     BlockId { height: 11, hash: hash_b },
//!     BlockId { height: 12, hash: hash_c },
//! ]);
//! let _ = chain
//!     .apply_update(update)
//!     .expect("we can evict/replace checkpoint 11 since it is the only checkpoint");
//!
//! // now our `chain` has 2 checkpoints (11:hash_b & 12:hash_c)
//! // we detect another reorg, this time at height 12...
//! let update = SparseChain::from_checkpoints(vec![
//!     // we connect at checkpoint 11 as this is our "point of agreement"
//!     BlockId { height: 11, hash: hash_b },
//!     BlockId { height: 12, hash: hash_d },
//! ]);
//! let _ = chain
//!     .apply_update(update)
//!     .expect("we have provided a valid point of agreement, so our reorg update will succeed");
//! ```
//!
//! ## Movement of transactions during update
//!
//! If the original sparsechain and update sparsechain contain the same transaction at different
//! [`ChainPosition`]s, the transaction is considered as "moved". There are various movements of a
//! transaction that are invalid and update will fail.
//!
//! Valid movements:
//!
//! * When the transaction moved from unconfirmed (in original) to confirmed (in update). In other
//!     words, confirming transactions are allowed!
//! * If there has been a reorg at height x, an originally confirmed transaction at height x or
//!     above, may move to another height (that is at x or above, including becoming unconfirmed).
//!
//! Invalid movements:
//!
//! * A confirmed transaction cannot move without a reorg.
//! * Even with a reorg, an originally confirmed transaction cannot be moved below the height of the
//!     reorg.
//!
//! # Custom [`ChainPosition`]
//!
//! [`SparseChain`] maintains a list of txids ordered by [`ChainPosition`]. By default, [`TxHeight`]
//! is used, however additional data can be incorporated into the implementation.
//!
//! For example, we can have "perfect ordering" of transactions if our positional index is a
//! combination of block height and transaction position in block.
//!
//! ```
//! # use bdk_chain::{BlockId, TxHeight, sparse_chain::*, example_utils::*};
//! # use bitcoin::{BlockHash, Txid};
//! # let hash_a = new_hash::<BlockHash>("a");
//! # let txid_1 = new_hash::<Txid>("1");
//! # let txid_2 = new_hash::<Txid>("2");
//! # let txid_3 = new_hash::<Txid>("3");
//! #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
//! pub enum TxPosition {
//!     Confirmed {
//!         height: u32,   // height of block
//!         position: u32, // position of transaction in the block
//!     },
//!     Unconfirmed,
//! }
//!
//! impl Default for TxPosition {
//!     fn default() -> Self { Self::Unconfirmed }
//! }
//!
//! impl ChainPosition for TxPosition {
//!     fn height(&self) -> TxHeight {
//!         match self {
//!             Self::Confirmed{ height, .. } => TxHeight::Confirmed(*height),
//!             Self::Unconfirmed => TxHeight::Unconfirmed,
//!         }
//!     }
//!
//!     fn max_ord_of_height(height: TxHeight) -> Self {
//!         match height {
//!             TxHeight::Confirmed(height) => Self::Confirmed{ height, position: u32::MAX },
//!             TxHeight::Unconfirmed => Self::Unconfirmed,
//!         }
//!     }
//!
//!     fn min_ord_of_height(height: TxHeight) -> Self {
//!         match height {
//!             TxHeight::Confirmed(height) => Self::Confirmed{ height, position: u32::MIN },
//!             TxHeight::Unconfirmed => Self::Unconfirmed,
//!         }
//!     }
//! }
//!
//! let mut chain = SparseChain::<TxPosition>::default();
//! let _ = chain.insert_checkpoint(BlockId { height: 10, hash: hash_a }).unwrap();
//! let _ = chain.insert_tx(txid_1, TxPosition::Confirmed{ height: 9, position: 4321 }).unwrap();
//! let _ = chain.insert_tx(txid_2, TxPosition::Confirmed{ height: 9, position: 1234 }).unwrap();
//! let _ = chain.insert_tx(txid_3, TxPosition::Confirmed{ height: 10, position: 321 }).unwrap();
//!
//! // transactions are ordered correctly
//! assert_eq!(
//!     chain.txids().collect::<Vec<_>>(),
//!     vec![
//!         &(TxPosition::Confirmed{ height: 9, position: 1234 }, txid_2),
//!         &(TxPosition::Confirmed{ height: 9, position: 4321 }, txid_1),
//!         &(TxPosition::Confirmed{ height: 10, position: 321 }, txid_3),
//!     ],
//! );
//!
//! ```
use core::{
    fmt::Debug,
    ops::{Bound, RangeBounds},
};

use crate::{collections::*, tx_graph::TxGraph, AsTransaction, BlockId, FullTxOut, TxHeight};
use bitcoin::{hashes::Hash, BlockHash, OutPoint, Txid};

/// This is a non-monotone structure that tracks relevant [`Txid`]s that are ordered by chain
/// position `P`.
///
/// We use [`BlockHash`]s alongside their chain height as "checkpoints" to enforce consistency.
///
/// To "merge" two [`SparseChain`]s, the [`ChangeSet`] can be calculated by calling
/// [`determine_changeset`] and applying the [`ChangeSet`] via [`apply_changeset`]. For convenience,
/// [`apply_update`] does the above two steps in one call.
///
/// Refer to [module-level documentation] for more.
///
/// [`determine_changeset`]: Self::determine_changeset
/// [`apply_changeset`]: Self::apply_changeset
/// [`apply_update`]: Self::apply_update
/// [module-level documentation]: crate::sparse_chain
#[derive(Clone, Debug, PartialEq)]
pub struct SparseChain<P = TxHeight> {
    /// Block height to checkpoint data.
    checkpoints: BTreeMap<u32, BlockHash>,
    /// Txids ordered by the pos `P`.
    ordered_txids: BTreeSet<(P, Txid)>,
    /// Confirmation heights of txids.
    txid_to_pos: HashMap<Txid, P>,
    /// Limit number of checkpoints.
    checkpoint_limit: Option<usize>,
}

impl<P> AsRef<SparseChain<P>> for SparseChain<P> {
    fn as_ref(&self) -> &SparseChain<P> {
        self
    }
}

impl<P> Default for SparseChain<P> {
    fn default() -> Self {
        Self {
            checkpoints: Default::default(),
            ordered_txids: Default::default(),
            txid_to_pos: Default::default(),
            checkpoint_limit: Default::default(),
        }
    }
}

/// Represents a failure when trying to insert a [`Txid`] into [`SparseChain`].
#[derive(Clone, Debug, PartialEq)]
pub enum InsertTxError<P> {
    /// Occurs when the [`Txid`] is to be inserted at a hight higher than the [`SparseChain`]'s tip.
    TxTooHigh {
        txid: Txid,
        tx_height: u32,
        tip_height: Option<u32>,
    },
    /// Occurs when the [`Txid`] is already in the [`SparseChain`] and the insertion would result in
    /// an unexpected move in [`ChainPosition`].
    TxMovedUnexpectedly {
        txid: Txid,
        original_pos: P,
        update_pos: P,
    },
}

impl<P: core::fmt::Debug> core::fmt::Display for InsertTxError<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InsertTxError::TxTooHigh {
                txid,
                tx_height,
                tip_height,
            } => write!(
                f,
                "txid ({}) cannot be inserted at height ({}) greater than chain tip ({:?})",
                txid, tx_height, tip_height
            ),
            InsertTxError::TxMovedUnexpectedly {
                txid,
                original_pos,
                update_pos,
            } => write!(
                f,
                "txid ({}) insertion resulted in an expected positional move from {:?} to {:?}",
                txid, original_pos, update_pos
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<P: core::fmt::Debug> std::error::Error for InsertTxError<P> {}

/// Represents a failure when trying to insert a checkpoint into [`SparseChain`].
#[derive(Clone, Debug, PartialEq)]
pub enum InsertCheckpointError {
    /// Occurs when checkpoint of the same height already exists with a different [`BlockHash`].
    HashNotMatching {
        height: u32,
        original_hash: BlockHash,
        update_hash: BlockHash,
    },
}

impl core::fmt::Display for InsertCheckpointError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InsertCheckpointError {}

/// Represents an update failure of [`SparseChain`].
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateError<P = TxHeight> {
    /// The update cannot be applied to the chain because the chain suffix it represents did not
    /// connect to the existing chain. This error case contains the checkpoint height to include so
    /// that the chains can connect.
    NotConnected(u32),
    /// The update contains inconsistent tx states (e.g. it changed the transaction's height). This
    /// error is usually the inconsistency found.
    TxInconsistent {
        txid: Txid,
        original_pos: P,
        update_pos: P,
    },
}

impl<P: core::fmt::Debug> core::fmt::Display for UpdateError<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotConnected(h) =>
                write!(f, "the checkpoints in the update could not be connected to the checkpoints in the chain, try include checkpoint of height {} to connect",
                    h),
            Self::TxInconsistent { txid, original_pos, update_pos } =>
                write!(f, "tx ({}) had position ({:?}), but is ({:?}) in the update",
                    txid, original_pos, update_pos),
        }
    }
}

#[cfg(feature = "std")]
impl<P: core::fmt::Debug> std::error::Error for UpdateError<P> {}

impl<P: ChainPosition> SparseChain<P> {
    /// Creates a new chain from a list of block hashes and heights. The caller must guarantee they
    /// are in the same chain.
    pub fn from_checkpoints<C>(checkpoints: C) -> Self
    where
        C: IntoIterator<Item = BlockId>,
    {
        let mut chain = Self::default();
        chain.checkpoints = checkpoints
            .into_iter()
            .map(|block_id| block_id.into())
            .collect();
        chain
    }

    /// Get the checkpoint for the last known tip.
    pub fn latest_checkpoint(&self) -> Option<BlockId> {
        self.checkpoints
            .iter()
            .last()
            .map(|(&height, &hash)| BlockId { height, hash })
    }

    /// Get the checkpoint at the given height if it exists.
    pub fn checkpoint_at(&self, height: u32) -> Option<BlockId> {
        self.checkpoints
            .get(&height)
            .map(|&hash| BlockId { height, hash })
    }

    /// Return the [`ChainPosition`] of a `txid`.
    ///
    /// This returns [`None`] if the transation does not exist.
    pub fn tx_position(&self, txid: Txid) -> Option<&P> {
        self.txid_to_pos.get(&txid)
    }

    /// Return a [`BTreeMap`] of all checkpoints (block hashes by height).
    pub fn checkpoints(&self) -> &BTreeMap<u32, BlockHash> {
        &self.checkpoints
    }

    /// Return an iterator over checkpoints in a height range, in ascending height order.
    pub fn range_checkpoints(
        &self,
        range: impl RangeBounds<u32>,
    ) -> impl DoubleEndedIterator<Item = BlockId> + '_ {
        self.checkpoints
            .range(range)
            .map(|(&height, &hash)| BlockId { height, hash })
    }

    /// Preview changes of updating [`Self`] with another chain that connects to it.
    ///
    /// If the `update` wishes to introduce confirmed transactions, it must contain a checkpoint
    /// that is exactly the same height as one of `self`'s checkpoints.
    ///
    /// To invalidate from a given checkpoint, `update` must contain a checkpoint of the same height
    /// but different hash. Invalidated checkpoints result in invalidated transactions becoming
    /// "unconfirmed".
    ///
    /// An error will be returned if an update will result in inconsistencies or if the update does
    /// not properly connect with `self`.
    ///
    /// Refer to [module-level documentation] for more.
    ///
    /// [module-level documentation]: crate::sparse_chain
    pub fn determine_changeset(&self, update: &Self) -> Result<ChangeSet<P>, UpdateError<P>> {
        let agreement_point = update
            .checkpoints
            .iter()
            .rev()
            .find(|&(height, hash)| self.checkpoints.get(height) == Some(hash))
            .map(|(&h, _)| h);

        let last_update_cp = update.checkpoints.iter().last().map(|(&h, _)| h);

        // the lower bound of the invalidation range
        let invalid_lb = if last_update_cp.is_none() || last_update_cp == agreement_point {
            // if agreement point is the last update checkpoint, or there is no update checkpoints,
            // no invalidation is required
            u32::MAX
        } else {
            agreement_point.map(|h| h + 1).unwrap_or(0)
        };

        // the first checkpoint of the sparsechain to invalidate (if any)
        let invalid_from = self.checkpoints.range(invalid_lb..).next().map(|(&h, _)| h);

        // the first checkpoint to invalidate (if any) should be represented in the update
        if let Some(first_invalid) = invalid_from {
            if !update.checkpoints.contains_key(&first_invalid) {
                return Err(UpdateError::NotConnected(first_invalid));
            }
        }

        for (&txid, update_pos) in &update.txid_to_pos {
            // ensure all currently confirmed txs are still at the same height (unless they are
            // within invalidation range, or to be confirmed)
            if let Some(original_pos) = &self.txid_to_pos.get(&txid) {
                if original_pos.height() < TxHeight::Confirmed(invalid_lb)
                    && original_pos != &update_pos
                {
                    return Err(UpdateError::TxInconsistent {
                        txid,
                        original_pos: P::clone(original_pos),
                        update_pos: update_pos.clone(),
                    });
                }
            }
        }

        // create initial change-set, based on checkpoints and txids that are to be "invalidated"
        let mut changeset = invalid_from
            .map(|from_height| self.invalidate_checkpoints_preview(from_height))
            .unwrap_or_default();

        for (&height, &new_hash) in &update.checkpoints {
            let original_hash = self.checkpoints.get(&height).cloned();

            let update_hash = *changeset
                .checkpoints
                .entry(height)
                .and_modify(|change| *change = Some(new_hash))
                .or_insert_with(|| Some(new_hash));

            if original_hash == update_hash {
                changeset.checkpoints.remove(&height);
            }
        }

        for (txid, new_pos) in &update.txid_to_pos {
            let original_pos = self.txid_to_pos.get(txid).cloned();

            let update_pos = changeset
                .txids
                .entry(*txid)
                .and_modify(|change| *change = Some(new_pos.clone()))
                .or_insert_with(|| Some(new_pos.clone()));

            if original_pos == *update_pos {
                changeset.txids.remove(txid);
            }
        }

        Ok(changeset)
    }

    /// Updates [`SparseChain`] with another chain that connects to it.
    ///
    /// This is equivilant to calling [`determine_changeset`] and [`apply_changeset`] in sequence.
    ///
    /// [`determine_changeset`]: Self::determine_changeset
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn apply_update(&mut self, update: Self) -> Result<ChangeSet<P>, UpdateError<P>> {
        let changeset = self.determine_changeset(&update)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    pub fn apply_changeset(&mut self, changeset: ChangeSet<P>) {
        for (height, update_hash) in changeset.checkpoints {
            let _original_hash = match update_hash {
                Some(update_hash) => self.checkpoints.insert(height, update_hash),
                None => self.checkpoints.remove(&height),
            };
        }

        for (txid, update_pos) in changeset.txids {
            let original_pos = self.txid_to_pos.remove(&txid);

            if let Some(pos) = original_pos {
                self.ordered_txids.remove(&(pos, txid));
            }

            if let Some(pos) = update_pos {
                self.txid_to_pos.insert(txid, pos.clone());
                self.ordered_txids.insert((pos.clone(), txid));
            }
        }

        self.prune_checkpoints();
    }

    /// Derives a [`ChangeSet`] that assumes that there are no preceding changesets.
    ///
    /// The changeset returned will record additions of all [`Txid`]s and checkpoints included in
    /// [`Self`].
    pub fn initial_changeset(&self) -> ChangeSet<P> {
        ChangeSet {
            checkpoints: self
                .checkpoints
                .iter()
                .map(|(height, hash)| (*height, Some(*hash)))
                .collect(),
            txids: self
                .ordered_txids
                .iter()
                .map(|(pos, txid)| (*txid, Some(pos.clone())))
                .collect(),
        }
    }

    /// Determines the [`ChangeSet`] when checkpoints `from_height` (inclusive) and above are
    /// invalidated. Displaced [`Txid`]s will be repositioned to [`TxHeight::Unconfirmed`].
    pub fn invalidate_checkpoints_preview(&self, from_height: u32) -> ChangeSet<P> {
        ChangeSet::<P> {
            checkpoints: self
                .checkpoints
                .range(from_height..)
                .map(|(height, _)| (*height, None))
                .collect(),
            // invalidated transactions become unconfirmed
            txids: self
                .range_txids_by_height(TxHeight::Confirmed(from_height)..TxHeight::Unconfirmed)
                .map(|(_, txid)| (*txid, Some(P::max_ord_of_height(TxHeight::Unconfirmed))))
                .collect(),
        }
    }

    /// Invalidate checkpoints `from_height` (inclusive) and above.
    ///
    /// This is equivalent to calling [`invalidate_checkpoints_preview`] and [`apply_changeset`] in
    /// sequence.
    ///
    /// [`invalidate_checkpoints_preview`]: Self::invalidate_checkpoints_preview
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn invalidate_checkpoints(&mut self, from_height: u32) -> ChangeSet<P> {
        let changeset = self.invalidate_checkpoints_preview(from_height);
        self.apply_changeset(changeset.clone());
        changeset
    }

    /// Determines the [`ChangeSet`] when all transactions of height [`TxHeight::Unconfirmed`] are
    /// removed completely.
    pub fn clear_mempool_preview(&self) -> ChangeSet<P> {
        let mempool_range = &(
            P::min_ord_of_height(TxHeight::Unconfirmed),
            Txid::all_zeros(),
        )..;

        let txids = self
            .ordered_txids
            .range(mempool_range)
            .map(|(_, txid)| (*txid, None))
            .collect();

        ChangeSet::<P> {
            txids,
            ..Default::default()
        }
    }

    /// Clears all transactions of height [`TxHeight::Unconfirmed`].
    ///
    /// This is equivalent to calling [`clear_mempool_preview`] and [`apply_changeset`] in sequence.
    ///
    /// [`clear_mempool_preview`]: Self::clear_mempool_preview
    /// [`apply_changeset`]: Self::apply_changeset
    /// [`ChangeSet`].
    pub fn clear_mempool(&mut self) -> ChangeSet<P> {
        let changeset = self.clear_mempool_preview();
        self.apply_changeset(changeset.clone());
        changeset
    }

    /// Determines the resultant [`ChangeSet`] if [`Txid`] was inserted at position `pos`.
    ///
    /// Changes to the [`Txid`]'s position is allowed (under the rules noted in
    /// [module-level documentation]) and will be reflected in the [`ChangeSet`].
    ///
    /// [module-level documentation]: crate::sparse_chain
    pub fn insert_tx_preview(&self, txid: Txid, pos: P) -> Result<ChangeSet<P>, InsertTxError<P>> {
        let mut update = Self::default();

        if let Some(block_id) = self.latest_checkpoint() {
            let _old_hash = update.checkpoints.insert(block_id.height, block_id.hash);
            debug_assert!(_old_hash.is_none());
        }

        let tip_height = self.checkpoints.iter().last().map(|(h, _)| *h);
        if let TxHeight::Confirmed(tx_height) = pos.height() {
            if Some(tx_height) > tip_height {
                return Err(InsertTxError::TxTooHigh {
                    txid,
                    tx_height,
                    tip_height,
                });
            }
        }

        let _old_pos = update.txid_to_pos.insert(txid, pos.clone());
        debug_assert!(_old_pos.is_none());

        let _inserted = update.ordered_txids.insert((pos, txid));
        debug_assert!(_inserted, "must insert tx");

        match self.determine_changeset(&update) {
            Ok(changeset) => Ok(changeset),
            Err(UpdateError::NotConnected(_)) => panic!("should always connect"),
            Err(UpdateError::TxInconsistent {
                txid: inconsistent_txid,
                original_pos,
                update_pos,
            }) => Err(InsertTxError::TxMovedUnexpectedly {
                txid: inconsistent_txid,
                original_pos,
                update_pos,
            }),
        }
    }

    /// Inserts a given [`Txid`] at `pos`.
    ///
    /// This is equivilant to calling [`insert_tx_preview`] and [`apply_changeset`] in sequence.
    ///
    /// [`insert_tx_preview`]: Self::insert_tx_preview
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn insert_tx(&mut self, txid: Txid, pos: P) -> Result<ChangeSet<P>, InsertTxError<P>> {
        let changeset = self.insert_tx_preview(txid, pos)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    /// Determines the resultant [`ChangeSet`] if [`BlockId`] was inserted.
    ///
    /// If the change would result in a change in block hash of a certain height, insertion would
    /// fail.
    pub fn insert_checkpoint_preview(
        &self,
        block_id: BlockId,
    ) -> Result<ChangeSet<P>, InsertCheckpointError> {
        let mut update = Self::default();

        if let Some(block_id) = self.latest_checkpoint() {
            let _old_hash = update.checkpoints.insert(block_id.height, block_id.hash);
            debug_assert!(_old_hash.is_none());
        }

        if let Some(original_hash) = update.checkpoints.insert(block_id.height, block_id.hash) {
            if original_hash != block_id.hash {
                return Err(InsertCheckpointError::HashNotMatching {
                    height: block_id.height,
                    original_hash,
                    update_hash: block_id.hash,
                });
            }
        }

        match self.determine_changeset(&update) {
            Ok(changeset) => Ok(changeset),
            Err(UpdateError::NotConnected(_)) => panic!("error should have caught above"),
            Err(UpdateError::TxInconsistent { .. }) => panic!("should never add txs"),
        }
    }

    /// Insert a checkpoint ([`BlockId`]).
    ///
    /// This is equivilant to calling [`insert_checkpoint_preview`] and [`apply_changeset`] in
    /// sequence.
    ///
    /// [`insert_checkpoint_preview`]: Self::insert_checkpoint_preview
    /// [`apply_changeset`]: Self::apply_changeset
    pub fn insert_checkpoint(
        &mut self,
        block_id: BlockId,
    ) -> Result<ChangeSet<P>, InsertCheckpointError> {
        let changeset = self.insert_checkpoint_preview(block_id)?;
        self.apply_changeset(changeset.clone());
        Ok(changeset)
    }

    /// Iterate over all [`Txid`]s ordered by their [`ChainPosition`].
    pub fn txids(&self) -> impl DoubleEndedIterator<Item = &(P, Txid)> + ExactSizeIterator + '_ {
        self.ordered_txids.iter()
    }

    /// Iterate over a sub-range of positioned [`Txid`]s.
    pub fn range_txids<R>(&self, range: R) -> impl DoubleEndedIterator<Item = &(P, Txid)> + '_
    where
        R: RangeBounds<(P, Txid)>,
    {
        let map_bound = |b: Bound<&(P, Txid)>| match b {
            Bound::Included((pos, txid)) => Bound::Included((pos.clone(), *txid)),
            Bound::Excluded((pos, txid)) => Bound::Excluded((pos.clone(), *txid)),
            Bound::Unbounded => Bound::Unbounded,
        };

        self.ordered_txids
            .range((map_bound(range.start_bound()), map_bound(range.end_bound())))
    }

    /// Iterate over a sub-range of positioned [`Txid`]s, where the range is defined by
    /// [`ChainPosition`] only.
    pub fn range_txids_by_position<R>(
        &self,
        range: R,
    ) -> impl DoubleEndedIterator<Item = &(P, Txid)> + '_
    where
        R: RangeBounds<P>,
    {
        let map_bound = |b: Bound<&P>, inc: Txid, exc: Txid| match b {
            Bound::Included(pos) => Bound::Included((pos.clone(), inc)),
            Bound::Excluded(pos) => Bound::Excluded((pos.clone(), exc)),
            Bound::Unbounded => Bound::Unbounded,
        };

        self.ordered_txids.range((
            map_bound(range.start_bound(), min_txid(), max_txid()),
            map_bound(range.end_bound(), max_txid(), min_txid()),
        ))
    }

    /// Iterate over a sub-range of positioned [`Txid`]s, where the range is define by [`TxHeight`]
    /// only.
    pub fn range_txids_by_height<R>(
        &self,
        range: R,
    ) -> impl DoubleEndedIterator<Item = &(P, Txid)> + '_
    where
        R: RangeBounds<TxHeight>,
    {
        let ord_it = |height, is_max| match is_max {
            true => P::max_ord_of_height(height),
            false => P::min_ord_of_height(height),
        };

        let map_bound = |b: Bound<&TxHeight>, inc: (bool, Txid), exc: (bool, Txid)| match b {
            Bound::Included(&h) => Bound::Included((ord_it(h, inc.0), inc.1)),
            Bound::Excluded(&h) => Bound::Excluded((ord_it(h, exc.0), exc.1)),
            Bound::Unbounded => Bound::Unbounded,
        };

        self.ordered_txids.range((
            map_bound(range.start_bound(), (false, min_txid()), (true, max_txid())),
            map_bound(range.end_bound(), (true, max_txid()), (false, min_txid())),
        ))
    }

    /// Attempt to retrieve a [`FullTxOut`] of the given `outpoint`.
    ///
    /// This will return `Some` only if the output's transaction is in both `self` and `graph`.
    pub fn full_txout(
        &self,
        graph: &TxGraph<impl AsTransaction>,
        outpoint: OutPoint,
    ) -> Option<FullTxOut<P>> {
        let chain_pos = self.tx_position(outpoint.txid)?;

        let tx = graph.get_tx(outpoint.txid)?;
        let is_on_coinbase = tx.as_tx().is_coin_base();
        let txout = tx.as_tx().output.get(outpoint.vout as usize)?.clone();

        let spent_by = self
            .spent_by(graph, outpoint)
            .map(|(pos, txid)| (pos.clone(), txid));

        Some(FullTxOut {
            outpoint,
            txout,
            chain_position: chain_pos.clone(),
            spent_by,
            is_on_coinbase,
        })
    }

    /// Returns the value set as the checkpoint limit.
    ///
    /// Refer to [`set_checkpoint_limit`].
    ///
    /// [`set_checkpoint_limit`]: Self::set_checkpoint_limit
    pub fn checkpoint_limit(&self) -> Option<usize> {
        self.checkpoint_limit
    }

    /// Set the checkpoint limit.
    ///
    /// The checkpoint limit restricts the number of checkpoints that can be stored in [`Self`].
    /// Oldest checkpoints are pruned first.
    pub fn set_checkpoint_limit(&mut self, limit: Option<usize>) {
        self.checkpoint_limit = limit;
        self.prune_checkpoints();
    }

    /// Return [`Txid`]s that would be added to the sparse chain if this `changeset` was applied.
    pub fn changeset_additions<'a>(
        &'a self,
        changeset: &'a ChangeSet<P>,
    ) -> impl Iterator<Item = Txid> + 'a {
        changeset
            .txids
            .iter()
            .filter(|(&txid, pos)| {
                pos.is_some() /*it was not a deletion*/ &&
                self.tx_position(txid).is_none() /*we don't have the txid already*/
            })
            .map(|(&txid, _)| txid)
    }

    fn prune_checkpoints(&mut self) -> Option<BTreeMap<u32, BlockHash>> {
        let limit = self.checkpoint_limit?;

        // find last height to be pruned
        let last_height = *self.checkpoints.keys().rev().nth(limit)?;
        // first height to be kept
        let keep_height = last_height + 1;

        let mut split = self.checkpoints.split_off(&keep_height);
        core::mem::swap(&mut self.checkpoints, &mut split);

        Some(split)
    }

    /// Finds the transaction in the chain that spends `outpoint`.
    ///
    /// [`TxGraph`] is used to provide the spend relationships.
    ///
    /// Note that the transaction including `outpoint` does not need to be in the `graph` or the
    /// `chain` for this to return `Some`.
    pub fn spent_by<T>(&self, graph: &TxGraph<T>, outpoint: OutPoint) -> Option<(&P, Txid)> {
        graph
            .outspends(outpoint)
            .iter()
            .find_map(|&txid| Some((self.tx_position(txid)?, txid)))
    }

    /// Returns whether the sparse chain contains any checkpoints or transactions.
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty() && self.txid_to_pos.is_empty()
    }
}

/// The return value of [`determine_changeset`].
///
/// [`determine_changeset`]: SparseChain::determine_changeset.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate")
)]
#[must_use]
pub struct ChangeSet<P = TxHeight> {
    pub checkpoints: BTreeMap<u32, Option<BlockHash>>,
    pub txids: BTreeMap<Txid, Option<P>>,
}

impl<I> Default for ChangeSet<I> {
    fn default() -> Self {
        Self {
            checkpoints: Default::default(),
            txids: Default::default(),
        }
    }
}

impl<P> ChangeSet<P> {
    /// Appends the changes in `other` into self such that applying `self` afterwards has the same
    /// effect as sequentially applying the original `self` and `other`.
    pub fn append(&mut self, mut other: Self)
    where
        P: ChainPosition,
    {
        self.checkpoints.append(&mut other.checkpoints);
        self.txids.append(&mut other.txids);
    }

    /// Whether this changeset contains no changes.
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty() && self.txids.is_empty()
    }
}

fn min_txid() -> Txid {
    Txid::from_inner([0x00; 32])
}

fn max_txid() -> Txid {
    Txid::from_inner([0xff; 32])
}

/// Represents an position in which transactions are ordered in [`SparseChain`].
///
/// [`ChainPosition`] implementations must be [`Ord`] by [`TxHeight`] first.
pub trait ChainPosition:
    core::fmt::Debug + Clone + Eq + PartialOrd + Ord + core::hash::Hash + Send + Sync + 'static
{
    /// Get the transaction height of the positon.
    fn height(&self) -> TxHeight;

    /// Get the positon's upper bound of a given height.
    fn max_ord_of_height(height: TxHeight) -> Self;

    /// Get the position's lower bound of a given height.
    fn min_ord_of_height(height: TxHeight) -> Self;

    /// Get the unconfirmed position.
    fn unconfirmed() -> Self {
        Self::max_ord_of_height(TxHeight::Unconfirmed)
    }
}

#[cfg(test)]
pub mod verify_chain_position {
    use crate::{sparse_chain::ChainPosition, ConfirmationTime, TxHeight};
    use alloc::vec::Vec;

    pub fn verify_chain_position<P: ChainPosition>(head_count: u32, tail_count: u32) {
        let values = (0..head_count)
            .chain(u32::MAX - tail_count..u32::MAX)
            .flat_map(|i| {
                [
                    P::min_ord_of_height(TxHeight::Confirmed(i)),
                    P::max_ord_of_height(TxHeight::Confirmed(i)),
                ]
            })
            .chain([
                P::min_ord_of_height(TxHeight::Unconfirmed),
                P::max_ord_of_height(TxHeight::Unconfirmed),
            ])
            .collect::<Vec<_>>();

        for i in 0..values.len() {
            for j in 0..values.len() {
                if i == j {
                    assert_eq!(values[i], values[j]);
                }
                if i < j {
                    assert!(values[i] <= values[j]);
                }
                if i > j {
                    assert!(values[i] >= values[j]);
                }
            }
        }
    }

    #[test]
    fn verify_tx_height() {
        verify_chain_position::<TxHeight>(1000, 1000);
    }

    #[test]
    fn verify_confirmation_time() {
        verify_chain_position::<ConfirmationTime>(1000, 1000);
    }
}
