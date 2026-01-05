use bitcoin::{constants::COINBASE_MATURITY, OutPoint, TxOut, Txid};

use crate::Anchor;

/// Represents the observed position of some chain data.
///
/// The generic `A` should be a [`Anchor`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, core::hash::Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(bound(
        deserialize = "A: Ord + serde::Deserialize<'de>",
        serialize = "A: Ord + serde::Serialize",
    ))
)]
pub enum ChainPosition<A> {
    /// The chain data is confirmed as it is anchored in the best chain by `A`.
    Confirmed {
        /// The [`Anchor`].
        anchor: A,
        /// Whether the chain data is anchored transitively by a child transaction.
        ///
        /// If the value is `Some`, it means we have incomplete data. We can only deduce that the
        /// chain data is confirmed at a block equal to or lower than the block referenced by `A`.
        transitively: Option<Txid>,
    },
    /// The chain data is not confirmed.
    Unconfirmed {
        /// When the chain data was first seen in the mempool.
        ///
        /// This value will be `None` if the chain data was never seen in the mempool.
        first_seen: Option<u64>,
        /// When the chain data is last seen in the mempool.
        ///
        /// This value will be `None` if the chain data was never seen in the mempool and only seen
        /// in a conflicting chain.
        last_seen: Option<u64>,
    },
}

impl<A> ChainPosition<A> {
    /// Returns whether [`ChainPosition`] is confirmed or not.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }

    /// Returns whether [`ChainPosition`] is unconfirmed or not.
    pub fn is_unconfirmed(&self) -> bool {
        matches!(self, Self::Unconfirmed { .. })
    }
}

impl<A: Clone> ChainPosition<&A> {
    /// Maps a [`ChainPosition<&A>`] into a [`ChainPosition<A>`] by cloning the contents.
    pub fn cloned(self) -> ChainPosition<A> {
        match self {
            ChainPosition::Confirmed {
                anchor,
                transitively,
            } => ChainPosition::Confirmed {
                anchor: anchor.clone(),
                transitively,
            },
            ChainPosition::Unconfirmed {
                last_seen,
                first_seen,
            } => ChainPosition::Unconfirmed {
                last_seen,
                first_seen,
            },
        }
    }
}

impl<A: Anchor> ChainPosition<A> {
    /// Determines the upper bound of the confirmation height.
    pub fn confirmation_height_upper_bound(&self) -> Option<u32> {
        match self {
            ChainPosition::Confirmed { anchor, .. } => {
                Some(anchor.confirmation_height_upper_bound())
            }
            ChainPosition::Unconfirmed { .. } => None,
        }
    }
}

/// Ordering for `ChainPosition`:
///
/// 1. Confirmed transactions come before unconfirmed
/// 2. Confirmed transactions are ordered by anchor (lower height = earlier)
/// 3. At equal anchor height, transitive confirmations come before direct
/// 4. Unconfirmed transactions with mempool timestamps come before those without
/// 5. Unconfirmed transactions are ordered by `first_seen`, then `last_seen`
impl<A: Ord> Ord for ChainPosition<A> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        use core::cmp::Ordering;

        /// Compares options where `None` is greater than `Some` (sorts last).
        fn cmp_none_last<T: Ord>(t1: &Option<T>, t2: &Option<T>) -> Ordering {
            match (t1, t2) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(t1), Some(t2)) => t1.cmp(t2),
            }
        }

        match (self, other) {
            // Both confirmed: compare by anchor first
            (
                ChainPosition::Confirmed {
                    anchor: a1,
                    transitively: t1,
                },
                ChainPosition::Confirmed {
                    anchor: a2,
                    transitively: t2,
                },
            ) => {
                // First compare anchors
                match a1.cmp(a2) {
                    // Same anchor: transitive before direct, tiebreak with txid
                    Ordering::Equal => cmp_none_last(t1, t2),
                    other => other,
                }
            }

            // Both unconfirmed: special handling for None values
            (
                ChainPosition::Unconfirmed {
                    first_seen: f1,
                    last_seen: l1,
                },
                ChainPosition::Unconfirmed {
                    first_seen: f2,
                    last_seen: l2,
                },
            ) => {
                // Never-in-mempool (None, None) ordered last
                // Compare by first_seen, tie-break with last_seen
                match cmp_none_last(f1, f2) {
                    Ordering::Equal => cmp_none_last(l1, l2),
                    other => other,
                }
            }

            // Confirmed always comes before unconfirmed
            (ChainPosition::Confirmed { .. }, ChainPosition::Unconfirmed { .. }) => Ordering::Less,
            (ChainPosition::Unconfirmed { .. }, ChainPosition::Confirmed { .. }) => {
                Ordering::Greater
            }
        }
    }
}

/// Partial ordering for `ChainPosition` - delegates to `Ord` implementation.
impl<A: Ord> PartialOrd for ChainPosition<A> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A `TxOut` with as much data as we can retrieve about it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullTxOut<A> {
    /// The position of the transaction in `outpoint` in the overall chain.
    pub chain_position: ChainPosition<A>,
    /// The location of the `TxOut`.
    pub outpoint: OutPoint,
    /// The `TxOut`.
    pub txout: TxOut,
    /// The txid and chain position of the transaction (if any) that has spent this output.
    pub spent_by: Option<(ChainPosition<A>, Txid)>,
    /// Whether this output is on a coinbase transaction.
    pub is_on_coinbase: bool,
}

impl<A: Ord> Ord for FullTxOut<A> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.chain_position
            .cmp(&other.chain_position)
            // Tie-break with `outpoint` and `spent_by`.
            .then_with(|| self.outpoint.cmp(&other.outpoint))
            .then_with(|| self.spent_by.cmp(&other.spent_by))
    }
}

impl<A: Ord> PartialOrd for FullTxOut<A> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Anchor> FullTxOut<A> {
    /// Whether the `txout` is considered mature.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_mature(&self, tip: u32) -> bool {
        if self.is_on_coinbase {
            let conf_height = match self.chain_position.confirmation_height_upper_bound() {
                Some(height) => height,
                None => {
                    debug_assert!(false, "coinbase tx can never be unconfirmed");
                    return false;
                }
            };
            let age = tip.saturating_sub(conf_height);
            if age + 1 < COINBASE_MATURITY {
                return false;
            }
        }

        true
    }

    /// Whether the utxo is/was/will be spendable with chain `tip`.
    ///
    /// This method does not take into account the lock time.
    ///
    /// Depending on the implementation of [`confirmation_height_upper_bound`] in [`Anchor`], this
    /// method may return false-negatives. In other words, interpreted confirmation count may be
    /// less than the actual value.
    ///
    /// [`confirmation_height_upper_bound`]: Anchor::confirmation_height_upper_bound
    pub fn is_confirmed_and_spendable(&self, tip: u32) -> bool {
        if !self.is_mature(tip) {
            return false;
        }

        let conf_height = match self.chain_position.confirmation_height_upper_bound() {
            Some(height) => height,
            None => return false,
        };
        if conf_height > tip {
            return false;
        }

        // if the spending tx is confirmed within tip height, the txout is no longer spendable
        if let Some(spend_height) = self
            .spent_by
            .as_ref()
            .and_then(|(pos, _)| pos.confirmation_height_upper_bound())
        {
            if spend_height <= tip {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use bdk_core::ConfirmationBlockTime;
    use bitcoin::hashes::Hash;

    use crate::BlockId;

    use super::*;

    #[test]
    fn chain_position_ord() {
        // Create test positions
        let conf_deep = ChainPosition::Confirmed {
            anchor: ConfirmationBlockTime {
                confirmation_time: 20,
                block_id: BlockId {
                    height: 9,
                    ..Default::default()
                },
            },
            transitively: None,
        };
        let conf_deep_transitive = ChainPosition::Confirmed {
            anchor: ConfirmationBlockTime {
                confirmation_time: 20,
                block_id: BlockId {
                    height: 9,
                    ..Default::default()
                },
            },
            transitively: Some(Txid::all_zeros()),
        };
        let conf_shallow = ChainPosition::Confirmed {
            anchor: ConfirmationBlockTime {
                confirmation_time: 15,
                block_id: BlockId {
                    height: 12,
                    ..Default::default()
                },
            },
            transitively: None,
        };
        let unconf_seen_early = ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
            first_seen: Some(10),
            last_seen: Some(10),
        };
        let unconf_seen_late = ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
            first_seen: Some(20),
            last_seen: Some(20),
        };
        let unconf_seen_early_and_late = ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
            first_seen: Some(10),
            last_seen: Some(20),
        };
        let unconf_never_seen = ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
            first_seen: None,
            last_seen: None,
        };

        // Test ordering: confirmed < unconfirmed
        assert!(
            conf_deep < unconf_seen_early,
            "confirmed comes before unconfirmed"
        );
        assert!(
            conf_shallow < unconf_seen_early,
            "confirmed comes before unconfirmed"
        );

        // Test ordering within confirmed: lower height (more confirmations) comes first
        assert!(
            conf_deep < conf_shallow,
            "deeper blocks (lower height) come first"
        );

        // Test ordering within confirmed at same height: transitive comes before direct
        assert!(
            conf_deep_transitive < conf_deep,
            "transitive confirmation comes before direct at same height"
        );

        // Test ordering within unconfirmed: earlier first_seen comes first, tie_break with
        // last_seen
        assert!(
            unconf_seen_early < unconf_seen_late,
            "earlier first_seen comes first"
        );
        assert!(
            unconf_seen_early < unconf_seen_early_and_late,
            "if first_seen is equal, tiebreak with last_seen"
        );

        // Test ordering: never seen in mempool comes last
        assert!(
            unconf_seen_early < unconf_never_seen,
            "seen in mempool comes before never seen"
        );
        assert!(
            unconf_seen_late < unconf_never_seen,
            "seen in mempool comes before never seen"
        );

        // Full ordering test: most confirmed -> least confirmed -> unconfirmed seen -> unconfirmed
        // never seen
        let mut positions = vec![
            unconf_never_seen,
            unconf_seen_late,
            conf_shallow,
            unconf_seen_early,
            conf_deep,
            conf_deep_transitive,
            unconf_seen_early_and_late,
        ];
        positions.sort();
        assert_eq!(
            positions,
            vec![
                conf_deep_transitive,       // Most confirmed (potentially)
                conf_deep,                  // Deep confirmation
                conf_shallow,               // Shallow confirmation
                unconf_seen_early,          // Unconfirmed, seen early
                unconf_seen_early_and_late, // Unconfirmed, seen early and late
                unconf_seen_late,           // Unconfirmed, seen late
                unconf_never_seen,          // Never in mempool
            ]
        );
    }

    #[test]
    fn test_sort_unconfirmed_chain_position() {
        let mut v = vec![
            ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                first_seen: Some(5),
                last_seen: Some(20),
            },
            ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                first_seen: Some(15),
                last_seen: Some(30),
            },
            ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                first_seen: Some(1),
                last_seen: Some(10),
            },
            ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                first_seen: Some(3),
                last_seen: Some(6),
            },
        ];

        v.sort();

        assert_eq!(
            v,
            vec![
                ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                    first_seen: Some(1),
                    last_seen: Some(10)
                },
                ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                    first_seen: Some(3),
                    last_seen: Some(6)
                },
                ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                    first_seen: Some(5),
                    last_seen: Some(20)
                },
                ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
                    first_seen: Some(15),
                    last_seen: Some(30)
                },
            ]
        );
    }
}
