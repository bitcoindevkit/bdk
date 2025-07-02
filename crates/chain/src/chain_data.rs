use bitcoin::{constants::COINBASE_MATURITY, OutPoint, TxOut, Txid};

use crate::Anchor;

/// Represents the observed position of some chain data.
///
/// The generic `A` should be a [`Anchor`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, core::hash::Hash)]
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

/// A `TxOut` with as much data as we can retrieve about it
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
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

    use crate::BlockId;

    use super::*;

    #[test]
    fn chain_position_ord() {
        let unconf1 = ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
            last_seen: Some(10),
            first_seen: Some(10),
        };
        let unconf2 = ChainPosition::<ConfirmationBlockTime>::Unconfirmed {
            last_seen: Some(20),
            first_seen: Some(20),
        };
        let conf1 = ChainPosition::Confirmed {
            anchor: ConfirmationBlockTime {
                confirmation_time: 20,
                block_id: BlockId {
                    height: 9,
                    ..Default::default()
                },
            },
            transitively: None,
        };
        let conf2 = ChainPosition::Confirmed {
            anchor: ConfirmationBlockTime {
                confirmation_time: 15,
                block_id: BlockId {
                    height: 12,
                    ..Default::default()
                },
            },
            transitively: None,
        };

        assert!(unconf2 > unconf1, "higher last_seen means higher ord");
        assert!(unconf1 > conf1, "unconfirmed is higher ord than confirmed");
        assert!(
            conf2 > conf1,
            "confirmation_height is higher then it should be higher ord"
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
