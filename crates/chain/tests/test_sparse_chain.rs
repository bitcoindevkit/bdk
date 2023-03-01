#[macro_use]
mod common;

use bdk_chain::{collections::BTreeSet, sparse_chain::*, BlockId, TxHeight};
use bitcoin::{hashes::Hash, Txid};
use core::ops::Bound;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct TestIndex(TxHeight, u32);

impl ChainPosition for TestIndex {
    fn height(&self) -> TxHeight {
        self.0
    }

    fn max_ord_of_height(height: TxHeight) -> Self {
        Self(height, u32::MAX)
    }

    fn min_ord_of_height(height: TxHeight) -> Self {
        Self(height, u32::MIN)
    }
}

impl TestIndex {
    pub fn new<H>(height: H, ext: u32) -> Self
    where
        H: Into<TxHeight>,
    {
        Self(height.into(), ext)
    }
}

#[test]
fn add_first_checkpoint() {
    let chain = SparseChain::default();
    assert_eq!(
        chain.determine_changeset(&chain!([0, h!("A")])),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("A")))],
            txids: []
        },),
        "add first tip"
    );
}

#[test]
fn add_second_tip() {
    let chain = chain!([0, h!("A")]);
    assert_eq!(
        chain.determine_changeset(&chain!([0, h!("A")], [1, h!("B")])),
        Ok(changeset! {
            checkpoints: [(1, Some(h!("B")))],
            txids: []
        },),
        "extend tip by one"
    );
}

#[test]
fn two_disjoint_chains_cannot_merge() {
    let chain1 = chain!([0, h!("A")]);
    let chain2 = chain!([1, h!("B")]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateError::NotConnected(0))
    );
}

#[test]
fn duplicate_chains_should_merge() {
    let chain1 = chain!([0, h!("A")]);
    let chain2 = chain!([0, h!("A")]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(ChangeSet::default())
    );
}

#[test]
fn duplicate_chains_with_txs_should_merge() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    let chain2 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(ChangeSet::default())
    );
}

#[test]
fn duplicate_chains_with_different_txs_should_merge() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    let chain2 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx1"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [],
            txids: [(h!("tx1"), Some(TxHeight::Confirmed(0)))]
        })
    );
}

#[test]
fn invalidate_first_and_only_checkpoint_without_tx_changes() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    let chain2 = chain!(checkpoints: [[0,h!("A'")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("A'")))],
            txids: []
        },)
    );
}

#[test]
fn invalidate_first_and_only_checkpoint_with_tx_move_forward() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    let chain2 = chain!(checkpoints: [[0,h!("A'")],[1, h!("B")]], txids: [(h!("tx0"), TxHeight::Confirmed(1))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("A'"))), (1, Some(h!("B")))],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(1)))]
        },)
    );
}

#[test]
fn invalidate_first_and_only_checkpoint_with_tx_move_backward() {
    let chain1 = chain!(checkpoints: [[1,h!("B")]], txids: [(h!("tx0"), TxHeight::Confirmed(1))]);
    let chain2 = chain!(checkpoints: [[0,h!("A")],[1, h!("B'")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("A"))), (1, Some(h!("B'")))],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(0)))]
        },)
    );
}

#[test]
fn invalidate_a_checkpoint_and_try_and_move_tx_when_it_wasnt_within_invalidation() {
    let chain1 = chain!(checkpoints: [[0, h!("A")], [1, h!("B")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    let chain2 = chain!(checkpoints: [[0, h!("A")], [1, h!("B'")]], txids: [(h!("tx0"), TxHeight::Confirmed(1))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateError::TxInconsistent {
            txid: h!("tx0"),
            original_pos: TxHeight::Confirmed(0).into(),
            update_pos: TxHeight::Confirmed(1).into(),
        })
    );
}

/// This test doesn't make much sense. We're invalidating a block at height 1 and moving it to
/// height 0. It should be impossible for it to be at height 1 at any point if it was at height 0
/// all along.
#[test]
fn move_invalidated_tx_into_earlier_checkpoint() {
    let chain1 = chain!(checkpoints: [[0, h!("A")], [1, h!("B")]], txids: [(h!("tx0"), TxHeight::Confirmed(1))]);
    let chain2 = chain!(checkpoints: [[0, h!("A")], [1, h!("B'")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(1, Some(h!("B'")))],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(0)))]
        },)
    );
}

#[test]
fn invalidate_first_and_only_checkpoint_with_tx_move_to_mempool() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    let chain2 = chain!(checkpoints: [[0,h!("A'")]], txids: [(h!("tx0"), TxHeight::Unconfirmed)]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("A'")))],
            txids: [(h!("tx0"), Some(TxHeight::Unconfirmed))]
        },)
    );
}

#[test]
fn confirm_tx_without_extending_chain() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Unconfirmed)]);
    let chain2 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(0)))]
        },)
    );
}

#[test]
fn confirm_tx_backwards_while_extending_chain() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Unconfirmed)]);
    let chain2 = chain!(checkpoints: [[0,h!("A")],[1,h!("B")]], txids: [(h!("tx0"), TxHeight::Confirmed(0))]);
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(1, Some(h!("B")))],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(0)))]
        },)
    );
}

#[test]
fn confirm_tx_in_new_block() {
    let chain1 = chain!(checkpoints: [[0,h!("A")]], txids: [(h!("tx0"), TxHeight::Unconfirmed)]);
    let chain2 = chain! {
        checkpoints: [[0,h!("A")], [1,h!("B")]],
        txids: [(h!("tx0"), TxHeight::Confirmed(1))]
    };
    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(1, Some(h!("B")))],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(1)))]
        },)
    );
}

#[test]
fn merging_mempool_of_empty_chains_doesnt_fail() {
    let chain1 = chain!(checkpoints: [], txids: [(h!("tx0"), TxHeight::Unconfirmed)]);
    let chain2 = chain!(checkpoints: [], txids: [(h!("tx1"), TxHeight::Unconfirmed)]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [],
            txids: [(h!("tx1"), Some(TxHeight::Unconfirmed))]
        },)
    );
}

#[test]
fn cannot_insert_confirmed_tx_without_checkpoints() {
    let chain = SparseChain::default();
    assert_eq!(
        chain.insert_tx_preview(h!("A"), TxHeight::Confirmed(0)),
        Err(InsertTxError::TxTooHigh {
            txid: h!("A"),
            tx_height: 0,
            tip_height: None
        })
    );
}

#[test]
fn empty_chain_can_add_unconfirmed_transactions() {
    let chain1 = chain!(checkpoints: [[0, h!("A")]], txids: []);
    let chain2 = chain!(checkpoints: [], txids: [(h!("tx0"), TxHeight::Unconfirmed)]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [],
            txids: [ (h!("tx0"), Some(TxHeight::Unconfirmed)) ]
        },)
    );
}

#[test]
fn can_update_with_shorter_chain() {
    let chain1 = chain!(checkpoints: [[1, h!("B")],[2, h!("C")]], txids: []);
    let chain2 = chain!(checkpoints: [[1, h!("B")]], txids: [(h!("tx0"), TxHeight::Confirmed(1))]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [],
            txids: [(h!("tx0"), Some(TxHeight::Confirmed(1)))]
        },)
    )
}

#[test]
fn can_introduce_older_checkpoints() {
    let chain1 = chain!(checkpoints: [[2, h!("C")], [3, h!("D")]], txids: []);
    let chain2 = chain!(checkpoints: [[1, h!("B")], [2, h!("C")]], txids: []);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(1, Some(h!("B")))],
            txids: []
        },)
    );
}

#[test]
fn fix_blockhash_before_agreement_point() {
    let chain1 = chain!([0, h!("im-wrong")], [1, h!("we-agree")]);
    let chain2 = chain!([0, h!("fix")], [1, h!("we-agree")]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("fix")))],
            txids: []
        },)
    )
}

// TODO: Use macro
#[test]
fn cannot_change_ext_index_of_confirmed_tx() {
    let chain1 = chain!(
        index: TestIndex,
        checkpoints: [[1, h!("A")]],
        txids: [(h!("tx0"), TestIndex(TxHeight::Confirmed(1), 10))]
    );
    let chain2 = chain!(
        index: TestIndex,
        checkpoints: [[1, h!("A")]],
        txids: [(h!("tx0"), TestIndex(TxHeight::Confirmed(1), 20))]
    );

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateError::TxInconsistent {
            txid: h!("tx0"),
            original_pos: TestIndex(TxHeight::Confirmed(1), 10),
            update_pos: TestIndex(TxHeight::Confirmed(1), 20),
        }),
    )
}

#[test]
fn can_change_index_of_unconfirmed_tx() {
    let chain1 = chain!(
        index: TestIndex,
        checkpoints: [[1, h!("A")]],
        txids: [(h!("tx1"), TestIndex(TxHeight::Unconfirmed, 10))]
    );
    let chain2 = chain!(
        index: TestIndex,
        checkpoints: [[1, h!("A")]],
        txids: [(h!("tx1"), TestIndex(TxHeight::Unconfirmed, 20))]
    );

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(ChangeSet {
            checkpoints: [].into(),
            txids: [(h!("tx1"), Some(TestIndex(TxHeight::Unconfirmed, 20)),)].into()
        },),
    )
}

/// B and C are in both chain and update
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  |     B   C
/// update | A   B   C   D
/// ```
/// This should succeed with the point of agreement being C and A should be added in addition.
#[test]
fn two_points_of_agreement() {
    let chain1 = chain!([1, h!("B")], [2, h!("C")]);
    let chain2 = chain!([0, h!("A")], [1, h!("B")], [2, h!("C")], [3, h!("D")]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [(0, Some(h!("A"))), (3, Some(h!("D")))]
        },),
    );
}

/// Update and chain does not connect:
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  |     B   C
/// update | A   B       D
/// ```
/// This should fail as we cannot figure out whether C & D are on the same chain
#[test]
fn update_and_chain_does_not_connect() {
    let chain1 = chain!([1, h!("B")], [2, h!("C")]);
    let chain2 = chain!([0, h!("A")], [1, h!("B")], [3, h!("D")]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateError::NotConnected(2)),
    );
}

/// Transient invalidation:
/// ```
///        | 0 | 1 | 2 | 3 | 4 | 5
/// chain  | A       B   C       E
/// update | A       B'  C'  D
/// ```
/// This should succeed and invalidate B,C and E with point of agreement being A.
/// It should also invalidate transactions at height 1.
#[test]
fn transitive_invalidation_applies_to_checkpoints_higher_than_invalidation() {
    let chain1 = chain! {
        checkpoints: [[0, h!("A")], [2, h!("B")], [3, h!("C")], [5, h!("E")]],
        txids: [
            (h!("a"), TxHeight::Confirmed(0)),
            (h!("b1"), TxHeight::Confirmed(1)),
            (h!("b2"), TxHeight::Confirmed(2)),
            (h!("d"), TxHeight::Confirmed(3)),
            (h!("e"), TxHeight::Confirmed(5))
        ]
    };
    let chain2 = chain! {
        checkpoints: [[0, h!("A")], [2, h!("B'")], [3, h!("C'")], [4, h!("D")]],
        txids: [(h!("b1"), TxHeight::Confirmed(4)), (h!("b2"), TxHeight::Confirmed(3))]
    };

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [
                (2, Some(h!("B'"))),
                (3, Some(h!("C'"))),
                (4, Some(h!("D"))),
                (5, None)
            ],
            txids: [
                (h!("b1"), Some(TxHeight::Confirmed(4))),
                (h!("b2"), Some(TxHeight::Confirmed(3))),
                (h!("d"), Some(TxHeight::Unconfirmed)),
                (h!("e"), Some(TxHeight::Unconfirmed))
            ]
        },)
    );
}

/// Transient invalidation:
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  |     B   C       E
/// update |     B'  C'  D
/// ```
///
/// This should succeed and invalidate B, C and E with no point of agreement
#[test]
fn transitive_invalidation_applies_to_checkpoints_higher_than_invalidation_no_point_of_agreement() {
    let chain1 = chain!([1, h!("B")], [2, h!("C")], [4, h!("E")]);
    let chain2 = chain!([1, h!("B'")], [2, h!("C'")], [3, h!("D")]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [
                (1, Some(h!("B'"))),
                (2, Some(h!("C'"))),
                (3, Some(h!("D"))),
                (4, None)
            ]
        },)
    )
}

/// Transient invalidation:
/// ```
///        | 0 | 1 | 2 | 3 | 4
/// chain  | A   B   C       E
/// update |     B'  C'  D
/// ```
///
/// This should fail since although it tells us that B and C are invalid it doesn't tell us whether
/// A was invalid.
#[test]
fn invalidation_but_no_connection() {
    let chain1 = chain!([0, h!("A")], [1, h!("B")], [2, h!("C")], [4, h!("E")]);
    let chain2 = chain!([1, h!("B'")], [2, h!("C'")], [3, h!("D")]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Err(UpdateError::NotConnected(0))
    )
}

#[test]
fn checkpoint_limit_is_respected() {
    let mut chain1 = SparseChain::default();
    let _ = chain1
        .apply_update(chain!(
            [1, h!("A")],
            [2, h!("B")],
            [3, h!("C")],
            [4, h!("D")],
            [5, h!("E")]
        ))
        .unwrap();

    assert_eq!(chain1.checkpoints().len(), 5);
    chain1.set_checkpoint_limit(Some(4));
    assert_eq!(chain1.checkpoints().len(), 4);

    let _ = chain1
        .insert_checkpoint(BlockId {
            height: 6,
            hash: h!("F"),
        })
        .unwrap();
    assert_eq!(chain1.checkpoints().len(), 4);

    let changeset = chain1.determine_changeset(&chain!([6, h!("F")], [7, h!("G")]));
    assert_eq!(changeset, Ok(changeset!(checkpoints: [(7, Some(h!("G")))])));

    chain1.apply_changeset(changeset.unwrap());

    assert_eq!(chain1.checkpoints().len(), 4);
}

#[test]
fn range_txids_by_height() {
    let mut chain = chain!(index: TestIndex, checkpoints: [[1, h!("block 1")], [2, h!("block 2")]]);

    let txids: [(TestIndex, Txid); 4] = [
        (
            TestIndex(TxHeight::Confirmed(1), u32::MIN),
            Txid::from_inner([0x00; 32]),
        ),
        (
            TestIndex(TxHeight::Confirmed(1), u32::MAX),
            Txid::from_inner([0xfe; 32]),
        ),
        (
            TestIndex(TxHeight::Confirmed(2), u32::MIN),
            Txid::from_inner([0x01; 32]),
        ),
        (
            TestIndex(TxHeight::Confirmed(2), u32::MAX),
            Txid::from_inner([0xff; 32]),
        ),
    ];

    // populate chain with txids
    for (index, txid) in txids {
        let _ = chain.insert_tx(txid, index).expect("should succeed");
    }

    // inclusive start
    assert_eq!(
        chain
            .range_txids_by_height(TxHeight::Confirmed(1)..)
            .collect::<Vec<_>>(),
        txids.iter().collect::<Vec<_>>(),
    );

    // exclusive start
    assert_eq!(
        chain
            .range_txids_by_height((Bound::Excluded(TxHeight::Confirmed(1)), Bound::Unbounded,))
            .collect::<Vec<_>>(),
        txids[2..].iter().collect::<Vec<_>>(),
    );

    // inclusive end
    assert_eq!(
        chain
            .range_txids_by_height((Bound::Unbounded, Bound::Included(TxHeight::Confirmed(2))))
            .collect::<Vec<_>>(),
        txids[..4].iter().collect::<Vec<_>>(),
    );

    // exclusive end
    assert_eq!(
        chain
            .range_txids_by_height(..TxHeight::Confirmed(2))
            .collect::<Vec<_>>(),
        txids[..2].iter().collect::<Vec<_>>(),
    );
}

#[test]
fn range_txids_by_index() {
    let mut chain = chain!(index: TestIndex, checkpoints: [[1, h!("block 1")],[2, h!("block 2")]]);

    let txids: [(TestIndex, Txid); 4] = [
        (TestIndex(TxHeight::Confirmed(1), u32::MIN), h!("tx 1 min")),
        (TestIndex(TxHeight::Confirmed(1), u32::MAX), h!("tx 1 max")),
        (TestIndex(TxHeight::Confirmed(2), u32::MIN), h!("tx 2 min")),
        (TestIndex(TxHeight::Confirmed(2), u32::MAX), h!("tx 2 max")),
    ];

    // populate chain with txids
    for (index, txid) in txids {
        let _ = chain.insert_tx(txid, index).expect("should succeed");
    }

    // inclusive start
    assert_eq!(
        chain
            .range_txids_by_position(TestIndex(TxHeight::Confirmed(1), u32::MIN)..)
            .collect::<Vec<_>>(),
        txids.iter().collect::<Vec<_>>(),
    );
    assert_eq!(
        chain
            .range_txids_by_position(TestIndex(TxHeight::Confirmed(1), u32::MAX)..)
            .collect::<Vec<_>>(),
        txids[1..].iter().collect::<Vec<_>>(),
    );

    // exclusive start
    assert_eq!(
        chain
            .range_txids_by_position((
                Bound::Excluded(TestIndex(TxHeight::Confirmed(1), u32::MIN)),
                Bound::Unbounded
            ))
            .collect::<Vec<_>>(),
        txids[1..].iter().collect::<Vec<_>>(),
    );
    assert_eq!(
        chain
            .range_txids_by_position((
                Bound::Excluded(TestIndex(TxHeight::Confirmed(1), u32::MAX)),
                Bound::Unbounded
            ))
            .collect::<Vec<_>>(),
        txids[2..].iter().collect::<Vec<_>>(),
    );

    // inclusive end
    assert_eq!(
        chain
            .range_txids_by_position((
                Bound::Unbounded,
                Bound::Included(TestIndex(TxHeight::Confirmed(2), u32::MIN))
            ))
            .collect::<Vec<_>>(),
        txids[..3].iter().collect::<Vec<_>>(),
    );
    assert_eq!(
        chain
            .range_txids_by_position((
                Bound::Unbounded,
                Bound::Included(TestIndex(TxHeight::Confirmed(2), u32::MAX))
            ))
            .collect::<Vec<_>>(),
        txids[..4].iter().collect::<Vec<_>>(),
    );

    // exclusive end
    assert_eq!(
        chain
            .range_txids_by_position(..TestIndex(TxHeight::Confirmed(2), u32::MIN))
            .collect::<Vec<_>>(),
        txids[..2].iter().collect::<Vec<_>>(),
    );
    assert_eq!(
        chain
            .range_txids_by_position(..TestIndex(TxHeight::Confirmed(2), u32::MAX))
            .collect::<Vec<_>>(),
        txids[..3].iter().collect::<Vec<_>>(),
    );
}

#[test]
fn range_txids() {
    let mut chain = SparseChain::default();

    let txids = (0..100)
        .map(|v| Txid::hash(v.to_string().as_bytes()))
        .collect::<BTreeSet<Txid>>();

    // populate chain
    for txid in &txids {
        let _ = chain
            .insert_tx(*txid, TxHeight::Unconfirmed)
            .expect("should succeed");
    }

    for txid in &txids {
        assert_eq!(
            chain
                .range_txids((TxHeight::Unconfirmed, *txid)..)
                .map(|(_, txid)| txid)
                .collect::<Vec<_>>(),
            txids.range(*txid..).collect::<Vec<_>>(),
            "range with inclusive start should succeed"
        );

        assert_eq!(
            chain
                .range_txids((
                    Bound::Excluded((TxHeight::Unconfirmed, *txid)),
                    Bound::Unbounded,
                ))
                .map(|(_, txid)| txid)
                .collect::<Vec<_>>(),
            txids
                .range((Bound::Excluded(*txid), Bound::Unbounded,))
                .collect::<Vec<_>>(),
            "range with exclusive start should succeed"
        );

        assert_eq!(
            chain
                .range_txids(..(TxHeight::Unconfirmed, *txid))
                .map(|(_, txid)| txid)
                .collect::<Vec<_>>(),
            txids.range(..*txid).collect::<Vec<_>>(),
            "range with exclusive end should succeed"
        );

        assert_eq!(
            chain
                .range_txids((
                    Bound::Included((TxHeight::Unconfirmed, *txid)),
                    Bound::Unbounded,
                ))
                .map(|(_, txid)| txid)
                .collect::<Vec<_>>(),
            txids
                .range((Bound::Included(*txid), Bound::Unbounded,))
                .collect::<Vec<_>>(),
            "range with inclusive end should succeed"
        );
    }
}

#[test]
fn invalidated_txs_move_to_unconfirmed() {
    let chain1 = chain! {
        checkpoints: [[0, h!("A")], [1, h!("B")], [2, h!("C")]],
        txids: [
            (h!("a"), TxHeight::Confirmed(0)),
            (h!("b"), TxHeight::Confirmed(1)),
            (h!("c"), TxHeight::Confirmed(2)),
            (h!("d"), TxHeight::Unconfirmed)
        ]
    };

    let chain2 = chain!([0, h!("A")], [1, h!("B'")]);

    assert_eq!(
        chain1.determine_changeset(&chain2),
        Ok(changeset! {
            checkpoints: [
                (1, Some(h!("B'"))),
                (2, None)
            ],
            txids: [
                (h!("b"), Some(TxHeight::Unconfirmed)),
                (h!("c"), Some(TxHeight::Unconfirmed))
            ]
        },)
    );
}

#[test]
fn change_tx_position_from_unconfirmed_to_confirmed() {
    let mut chain = SparseChain::<TxHeight>::default();
    let txid = h!("txid");

    let _ = chain.insert_tx(txid, TxHeight::Unconfirmed).unwrap();

    assert_eq!(chain.tx_position(txid), Some(&TxHeight::Unconfirmed));
    let _ = chain
        .insert_checkpoint(BlockId {
            height: 0,
            hash: h!("0"),
        })
        .unwrap();
    let _ = chain.insert_tx(txid, TxHeight::Confirmed(0)).unwrap();

    assert_eq!(chain.tx_position(txid), Some(&TxHeight::Confirmed(0)));
}
