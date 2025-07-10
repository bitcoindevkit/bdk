#![cfg(feature = "miniscript")]

use std::ops::{Bound, RangeBounds};

use bdk_chain::{
    local_chain::{
        AlterCheckPointError, ApplyHeaderError, CannotConnectError, ChangeSet, CheckPoint,
        LocalChain, MissingGenesisError,
    },
    BlockId,
};
use bdk_testenv::{chain_update, hash, local_chain};
use bitcoin::{block::Header, hashes::Hash, BlockHash};
use proptest::prelude::*;

#[derive(Debug)]
struct TestLocalChain<'a> {
    name: &'static str,
    chain: LocalChain,
    update: CheckPoint,
    exp: ExpectedResult<'a>,
}

#[derive(Debug, PartialEq)]
enum ExpectedResult<'a> {
    Ok {
        changeset: &'a [(u32, Option<BlockHash>)],
        init_changeset: &'a [(u32, Option<BlockHash>)],
    },
    Err(CannotConnectError),
}

impl TestLocalChain<'_> {
    fn run(mut self) {
        let got_changeset = match self.chain.apply_update(self.update) {
            Ok(changeset) => changeset,
            Err(got_err) => {
                assert_eq!(
                    ExpectedResult::Err(got_err),
                    self.exp,
                    "{}: unexpected error",
                    self.name
                );
                return;
            }
        };

        match self.exp {
            ExpectedResult::Ok {
                changeset,
                init_changeset,
            } => {
                assert_eq!(
                    got_changeset,
                    changeset.iter().cloned().collect(),
                    "{}: unexpected changeset",
                    self.name
                );
                assert_eq!(
                    self.chain.initial_changeset(),
                    init_changeset.iter().cloned().collect(),
                    "{}: unexpected initial changeset",
                    self.name
                );
            }
            ExpectedResult::Err(err) => panic!(
                "{}: expected error ({}), got non-error result: {:?}",
                self.name, err, got_changeset
            ),
        }
    }
}

#[test]
fn update_local_chain() {
    [
        TestLocalChain {
            name: "add first tip",
            chain: local_chain![(0, hash!("A"))],
            update: chain_update![(0, hash!("A"))],
            exp: ExpectedResult::Ok {
                changeset: &[],
                init_changeset: &[(0, Some(hash!("A")))],
            },
        },
        TestLocalChain {
            name: "add second tip",
            chain: local_chain![(0, hash!("A"))],
            update: chain_update![(0, hash!("A")), (1, hash!("B"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(hash!("B")))],
                init_changeset: &[(0, Some(hash!("A"))), (1, Some(hash!("B")))],
            },
        },
        TestLocalChain {
            name: "two disjoint chains cannot merge",
            chain: local_chain![(0, hash!("_")), (1, hash!("A"))],
            update: chain_update![(0, hash!("_")), (2, hash!("B"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 1,
            }),
        },
        TestLocalChain {
            name: "two disjoint chains cannot merge (existing chain longer)",
            chain: local_chain![(0, hash!("_")), (2, hash!("A"))],
            update: chain_update![(0, hash!("_")), (1, hash!("B"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 2,
            }),
        },
        TestLocalChain {
            name: "duplicate chains should merge",
            chain: local_chain![(0, hash!("A"))],
            update: chain_update![(0, hash!("A"))],
            exp: ExpectedResult::Ok {
                changeset: &[],
                init_changeset: &[(0, Some(hash!("A")))],
            },
        },
        // Introduce an older checkpoint (B)
        //        | 0 | 1 | 2 | 3
        // chain  | _       C   D
        // update | _   B   C
        TestLocalChain {
            name: "can introduce older checkpoint",
            chain: local_chain![(0, hash!("_")), (2, hash!("C")), (3, hash!("D"))],
            update: chain_update![(0, hash!("_")), (1, hash!("B")), (2, hash!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(hash!("B")))],
                init_changeset: &[(0, Some(hash!("_"))), (1, Some(hash!("B"))), (2, Some(hash!("C"))), (3, Some(hash!("D")))],
            },
        },
        // Introduce an older checkpoint (A) that is not directly behind PoA
        //        | 0 | 2 | 3 | 4
        // chain  | _       B   C
        // update | _   A       C
        TestLocalChain {
            name: "can introduce older checkpoint 2",
            chain: local_chain![(0, hash!("_")), (3, hash!("B")), (4, hash!("C"))],
            update: chain_update![(0, hash!("_")), (2, hash!("A")), (4, hash!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(2, Some(hash!("A")))],
                init_changeset: &[(0, Some(hash!("_"))), (2, Some(hash!("A"))), (3, Some(hash!("B"))), (4, Some(hash!("C")))],
            }
        },
        // Introduce an older checkpoint (B) that is not the oldest checkpoint
        //        | 0 | 1 | 2 | 3
        // chain  | _   A       C
        // update | _       B   C
        TestLocalChain {
            name: "can introduce older checkpoint 3",
            chain: local_chain![(0, hash!("_")), (1, hash!("A")), (3, hash!("C"))],
            update: chain_update![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(2, Some(hash!("B")))],
                init_changeset: &[(0, Some(hash!("_"))), (1, Some(hash!("A"))), (2, Some(hash!("B"))), (3, Some(hash!("C")))],
            }
        },
        // Introduce two older checkpoints below the PoA
        //        | 0 | 1 | 2 | 3
        // chain  | _           C
        // update | _   A   B   C
        TestLocalChain {
            name: "introduce two older checkpoints below PoA",
            chain: local_chain![(0, hash!("_")), (3, hash!("C"))],
            update: chain_update![(0, hash!("_")), (1, hash!("A")), (2, hash!("B")), (3, hash!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(hash!("A"))), (2, Some(hash!("B")))],
                init_changeset: &[(0, Some(hash!("_"))), (1, Some(hash!("A"))), (2, Some(hash!("B"))), (3, Some(hash!("C")))],
            },
        },
        TestLocalChain {
            name: "fix blockhash before agreement point",
            chain: local_chain![(0, hash!("im-wrong")), (1, hash!("we-agree"))],
            update: chain_update![(0, hash!("fix")), (1, hash!("we-agree"))],
            exp: ExpectedResult::Ok {
                changeset: &[(0, Some(hash!("fix")))],
                init_changeset: &[(0, Some(hash!("fix"))), (1, Some(hash!("we-agree")))],
            },
        },
        // B and C are in both chain and update
        //        | 0 | 1 | 2 | 3 | 4
        // chain  | _       B   C
        // update | _   A   B   C   D
        // This should succeed with the point of agreement being C and A should be added in addition.
        TestLocalChain {
            name: "two points of agreement",
            chain: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
            update: chain_update![(0, hash!("_")), (1, hash!("A")), (2, hash!("B")), (3, hash!("C")), (4, hash!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(hash!("A"))), (4, Some(hash!("D")))],
                init_changeset: &[
                    (0, Some(hash!("_"))),
                    (1, Some(hash!("A"))),
                    (2, Some(hash!("B"))),
                    (3, Some(hash!("C"))),
                    (4, Some(hash!("D"))),
                ],
            },
        },
        // Update and chain does not connect:
        //        | 0 | 1 | 2 | 3 | 4
        // chain  | _       B   C
        // update | _   A   B       D
        // This should fail as we cannot figure out whether C & D are on the same chain
        TestLocalChain {
            name: "update and chain does not connect",
            chain: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
            update: chain_update![(0, hash!("_")), (1, hash!("A")), (2, hash!("B")), (4, hash!("D"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 3,
            }),
        },
        // Transient invalidation:
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | _       B   C       E
        // update | _       B'  C'  D
        // This should succeed and invalidate B,C and E with point of agreement being A.
        TestLocalChain {
            name: "transitive invalidation applies to checkpoints higher than invalidation",
            chain: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C")), (5, hash!("E"))],
            update: chain_update![(0, hash!("_")), (2, hash!("B'")), (3, hash!("C'")), (4, hash!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (2, Some(hash!("B'"))),
                    (3, Some(hash!("C'"))),
                    (4, Some(hash!("D"))),
                    (5, None),
                ],
                init_changeset: &[
                    (0, Some(hash!("_"))),
                    (2, Some(hash!("B'"))),
                    (3, Some(hash!("C'"))),
                    (4, Some(hash!("D"))),
                ],
            },
        },
        // Transient invalidation:
        //        | 0 | 1 | 2 | 3 | 4
        // chain  | _   B   C       E
        // update | _   B'  C'  D
        // This should succeed and invalidate B, C and E with no point of agreement
        TestLocalChain {
            name: "transitive invalidation applies to checkpoints higher than invalidation no point of agreement",
            chain: local_chain![(0, hash!("_")), (1, hash!("B")), (2, hash!("C")), (4, hash!("E"))],
            update: chain_update![(0, hash!("_")), (1, hash!("B'")), (2, hash!("C'")), (3, hash!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (1, Some(hash!("B'"))),
                    (2, Some(hash!("C'"))),
                    (3, Some(hash!("D"))),
                    (4, None)
                ],
                init_changeset: &[
                    (0, Some(hash!("_"))),
                    (1, Some(hash!("B'"))),
                    (2, Some(hash!("C'"))),
                    (3, Some(hash!("D"))),
                ],
            },
        },
        // Transient invalidation:
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | _   A   B   C       E
        // update | _       B'  C'  D
        // This should fail since although it tells us that B and C are invalid it doesn't tell us whether
        // A was invalid.
        TestLocalChain {
            name: "invalidation but no connection",
            chain: local_chain![(0, hash!("_")), (1, hash!("A")), (2, hash!("B")), (3, hash!("C")), (5, hash!("E"))],
            update: chain_update![(0, hash!("_")), (2, hash!("B'")), (3, hash!("C'")), (4, hash!("D"))],
            exp: ExpectedResult::Err(CannotConnectError { try_include_height: 1 }),
        },
        // Introduce blocks between two points of agreement
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | A   B       D   E
        // update | A       C       E   F
        TestLocalChain {
            name: "introduce blocks between two points of agreement",
            chain: local_chain![(0, hash!("A")), (1, hash!("B")), (3, hash!("D")), (4, hash!("E"))],
            update: chain_update![(0, hash!("A")), (2, hash!("C")), (4, hash!("E")), (5, hash!("F"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (2, Some(hash!("C"))),
                    (5, Some(hash!("F"))),
                ],
                init_changeset: &[
                    (0, Some(hash!("A"))),
                    (1, Some(hash!("B"))),
                    (2, Some(hash!("C"))),
                    (3, Some(hash!("D"))),
                    (4, Some(hash!("E"))),
                    (5, Some(hash!("F"))),
                ],
            },
        },
        // Allow update that is shorter than original chain
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | A       C   D   E   F
        // update | A       C   D'
        TestLocalChain {
            name: "allow update that is shorter than original chain",
            chain: local_chain![(0, hash!("_")), (2, hash!("C")), (3, hash!("D")), (4, hash!("E")), (5, hash!("F"))],
            update: chain_update![(0, hash!("_")), (2, hash!("C")), (3, hash!("D'"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (3, Some(hash!("D'"))),
                    (4, None),
                    (5, None),
                ],
                init_changeset: &[
                    (0, Some(hash!("_"))),
                    (2, Some(hash!("C"))),
                    (3, Some(hash!("D'"))),
                ],
            },
        },
    ]
    .into_iter()
    .for_each(TestLocalChain::run);
}

#[test]
fn local_chain_insert_block() {
    struct TestCase {
        original: LocalChain,
        insert: (u32, BlockHash),
        expected_result: Result<ChangeSet, AlterCheckPointError>,
        expected_final: LocalChain,
    }

    let test_cases = [
        TestCase {
            original: local_chain![(0, hash!("_"))],
            insert: (5, hash!("block5")),
            expected_result: Ok([(5, Some(hash!("block5")))].into()),
            expected_final: local_chain![(0, hash!("_")), (5, hash!("block5"))],
        },
        TestCase {
            original: local_chain![(0, hash!("_")), (3, hash!("A"))],
            insert: (4, hash!("B")),
            expected_result: Ok([(4, Some(hash!("B")))].into()),
            expected_final: local_chain![(0, hash!("_")), (3, hash!("A")), (4, hash!("B"))],
        },
        TestCase {
            original: local_chain![(0, hash!("_")), (4, hash!("B"))],
            insert: (3, hash!("A")),
            expected_result: Ok([(3, Some(hash!("A")))].into()),
            expected_final: local_chain![(0, hash!("_")), (3, hash!("A")), (4, hash!("B"))],
        },
        TestCase {
            original: local_chain![(0, hash!("_")), (2, hash!("K"))],
            insert: (2, hash!("K")),
            expected_result: Ok([].into()),
            expected_final: local_chain![(0, hash!("_")), (2, hash!("K"))],
        },
        TestCase {
            original: local_chain![(0, hash!("_")), (2, hash!("K"))],
            insert: (2, hash!("J")),
            expected_result: Err(AlterCheckPointError {
                height: 2,
                original_hash: hash!("K"),
                update_hash: Some(hash!("J")),
            }),
            expected_final: local_chain![(0, hash!("_")), (2, hash!("K"))],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        let mut chain = t.original;
        assert_eq!(
            chain.insert_block(t.insert.into()),
            t.expected_result,
            "[{i}] unexpected result when inserting block",
        );
        assert_eq!(chain, t.expected_final, "[{i}] unexpected final chain",);
    }
}

#[test]
fn local_chain_disconnect_from() {
    struct TestCase {
        name: &'static str,
        original: LocalChain,
        disconnect_from: (u32, BlockHash),
        exp_result: Result<ChangeSet, MissingGenesisError>,
        exp_final: LocalChain,
    }

    let test_cases = [
        TestCase {
            name: "try_replace_genesis_should_fail",
            original: local_chain![(0, hash!("_"))],
            disconnect_from: (0, hash!("_")),
            exp_result: Err(MissingGenesisError),
            exp_final: local_chain![(0, hash!("_"))],
        },
        TestCase {
            name: "try_replace_genesis_should_fail_2",
            original: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
            disconnect_from: (0, hash!("_")),
            exp_result: Err(MissingGenesisError),
            exp_final: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
        },
        TestCase {
            name: "from_does_not_exist",
            original: local_chain![(0, hash!("_")), (3, hash!("C"))],
            disconnect_from: (2, hash!("B")),
            exp_result: Ok(ChangeSet::default()),
            exp_final: local_chain![(0, hash!("_")), (3, hash!("C"))],
        },
        TestCase {
            name: "from_has_different_blockhash",
            original: local_chain![(0, hash!("_")), (2, hash!("B"))],
            disconnect_from: (2, hash!("not_B")),
            exp_result: Ok(ChangeSet::default()),
            exp_final: local_chain![(0, hash!("_")), (2, hash!("B"))],
        },
        TestCase {
            name: "disconnect_one",
            original: local_chain![(0, hash!("_")), (2, hash!("B"))],
            disconnect_from: (2, hash!("B")),
            exp_result: Ok(ChangeSet::from_iter([(2, None)])),
            exp_final: local_chain![(0, hash!("_"))],
        },
        TestCase {
            name: "disconnect_three",
            original: local_chain![
                (0, hash!("_")),
                (2, hash!("B")),
                (3, hash!("C")),
                (4, hash!("D"))
            ],
            disconnect_from: (2, hash!("B")),
            exp_result: Ok(ChangeSet::from_iter([(2, None), (3, None), (4, None)])),
            exp_final: local_chain![(0, hash!("_"))],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        let mut chain = t.original;
        let result = chain.disconnect_from(t.disconnect_from.into());
        assert_eq!(
            result, t.exp_result,
            "[{}:{}] unexpected changeset result",
            i, t.name
        );
        assert_eq!(
            chain, t.exp_final,
            "[{}:{}] unexpected final chain",
            i, t.name
        );
    }
}

#[test]
fn checkpoint_from_block_ids() {
    struct TestCase<'a> {
        name: &'a str,
        blocks: &'a [(u32, BlockHash)],
        exp_result: Result<(), Option<(u32, BlockHash)>>,
    }

    let test_cases = [
        TestCase {
            name: "in_order",
            blocks: &[(0, hash!("A")), (1, hash!("B")), (3, hash!("D"))],
            exp_result: Ok(()),
        },
        TestCase {
            name: "with_duplicates",
            blocks: &[(1, hash!("B")), (2, hash!("C")), (2, hash!("C'"))],
            exp_result: Err(Some((2, hash!("C")))),
        },
        TestCase {
            name: "not_in_order",
            blocks: &[(1, hash!("B")), (3, hash!("D")), (2, hash!("C"))],
            exp_result: Err(Some((3, hash!("D")))),
        },
        TestCase {
            name: "empty",
            blocks: &[],
            exp_result: Err(None),
        },
        TestCase {
            name: "single",
            blocks: &[(21, hash!("million"))],
            exp_result: Ok(()),
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        let result = CheckPoint::from_block_ids(
            t.blocks
                .iter()
                .map(|&(height, hash)| BlockId { height, hash }),
        );
        match t.exp_result {
            Ok(_) => {
                assert!(result.is_ok(), "[{}:{}] should be Ok", i, t.name);
                let result_vec = {
                    let mut v = result
                        .unwrap()
                        .into_iter()
                        .map(|cp| (cp.height(), cp.hash()))
                        .collect::<Vec<_>>();
                    v.reverse();
                    v
                };
                assert_eq!(
                    &result_vec, t.blocks,
                    "[{}:{}] not equal to original block ids",
                    i, t.name
                );
            }
            Err(exp_last) => {
                assert!(result.is_err(), "[{}:{}] should be Err", i, t.name);
                let err = result.unwrap_err();
                assert_eq!(
                    err.as_ref()
                        .map(|last_cp| (last_cp.height(), last_cp.hash())),
                    exp_last,
                    "[{}:{}] error's last cp height should be {:?}, got {:?}",
                    i,
                    t.name,
                    exp_last,
                    err
                );
            }
        }
    }
}

#[test]
fn checkpoint_query() {
    struct TestCase {
        chain: LocalChain,
        /// The heights we want to call [`CheckPoint::query`] with, represented as an inclusive
        /// range.
        ///
        /// If a [`CheckPoint`] exists at that height, we expect [`CheckPoint::query`] to return
        /// it. If not, [`CheckPoint::query`] should return `None`.
        query_range: (u32, u32),
    }

    let test_cases = [
        TestCase {
            chain: local_chain![(0, hash!("_")), (1, hash!("A"))],
            query_range: (0, 2),
        },
        TestCase {
            chain: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
            query_range: (0, 3),
        },
    ];

    for t in test_cases.into_iter() {
        let tip = t.chain.tip();
        for h in t.query_range.0..=t.query_range.1 {
            let query_result = tip.get(h);

            // perform an exhausitive search for the checkpoint at height `h`
            let exp_hash = t
                .chain
                .iter_checkpoints()
                .find(|cp| cp.height() == h)
                .map(|cp| cp.hash());

            match query_result {
                Some(cp) => {
                    assert_eq!(Some(cp.hash()), exp_hash);
                    assert_eq!(cp.height(), h);
                }
                None => assert!(exp_hash.is_none()),
            }
        }
    }
}

#[test]
fn checkpoint_insert() {
    struct TestCase<'a> {
        /// The name of the test.
        #[allow(dead_code)]
        name: &'a str,
        /// The original checkpoint chain to call [`CheckPoint::insert`] on.
        chain: &'a [(u32, BlockHash)],
        /// The `block_id` to insert.
        to_insert: (u32, BlockHash),
        /// The expected final checkpoint chain after calling [`CheckPoint::insert`].
        exp_final_chain: &'a [(u32, BlockHash)],
    }

    let test_cases = [
        TestCase {
            name: "insert_above_tip",
            chain: &[(1, hash!("a")), (2, hash!("b"))],
            to_insert: (4, hash!("d")),
            exp_final_chain: &[(1, hash!("a")), (2, hash!("b")), (4, hash!("d"))],
        },
        TestCase {
            name: "insert_already_exists_expect_no_change",
            chain: &[(1, hash!("a")), (2, hash!("b")), (3, hash!("c"))],
            to_insert: (2, hash!("b")),
            exp_final_chain: &[(1, hash!("a")), (2, hash!("b")), (3, hash!("c"))],
        },
        TestCase {
            name: "insert_in_middle",
            chain: &[(2, hash!("b")), (4, hash!("d")), (5, hash!("e"))],
            to_insert: (3, hash!("c")),
            exp_final_chain: &[
                (2, hash!("b")),
                (3, hash!("c")),
                (4, hash!("d")),
                (5, hash!("e")),
            ],
        },
        TestCase {
            name: "replace_one",
            chain: &[(3, hash!("c")), (4, hash!("d")), (5, hash!("e"))],
            to_insert: (5, hash!("E")),
            exp_final_chain: &[(3, hash!("c")), (4, hash!("d")), (5, hash!("E"))],
        },
        TestCase {
            name: "insert_conflict_should_evict",
            chain: &[
                (3, hash!("c")),
                (4, hash!("d")),
                (5, hash!("e")),
                (6, hash!("f")),
            ],
            to_insert: (4, hash!("D")),
            exp_final_chain: &[(3, hash!("c")), (4, hash!("D"))],
        },
    ];

    fn genesis_block() -> impl Iterator<Item = BlockId> {
        core::iter::once((0, hash!("_"))).map(BlockId::from)
    }

    for t in test_cases.into_iter() {
        let chain = CheckPoint::from_block_ids(
            genesis_block().chain(t.chain.iter().copied().map(BlockId::from)),
        )
        .expect("test formed incorrectly, must construct checkpoint chain");

        let exp_final_chain = CheckPoint::from_block_ids(
            genesis_block().chain(t.exp_final_chain.iter().copied().map(BlockId::from)),
        )
        .expect("test formed incorrectly, must construct checkpoint chain");

        assert_eq!(
            chain.insert(t.to_insert.into()),
            exp_final_chain,
            "unexpected final chain"
        );
    }
}

#[test]
fn local_chain_apply_header_connected_to() {
    fn header_from_prev_blockhash(prev_blockhash: BlockHash) -> Header {
        Header {
            version: bitcoin::block::Version::default(),
            prev_blockhash,
            merkle_root: bitcoin::hash_types::TxMerkleNode::all_zeros(),
            time: 0,
            bits: bitcoin::CompactTarget::default(),
            nonce: 0,
        }
    }

    struct TestCase {
        name: &'static str,
        chain: LocalChain,
        header: Header,
        height: u32,
        connected_to: BlockId,
        exp_result: Result<Vec<(u32, Option<BlockHash>)>, ApplyHeaderError>,
    }

    let test_cases = [
        {
            let header = header_from_prev_blockhash(hash!("_"));
            let hash = header.block_hash();
            let height = 1;
            let connected_to = BlockId { height, hash };
            TestCase {
                name: "connected_to_self_header_applied_to_self",
                chain: local_chain![(0, hash!("_")), (height, hash)],
                header,
                height,
                connected_to,
                exp_result: Ok(vec![]),
            }
        },
        {
            let prev_hash = hash!("A");
            let prev_height = 1;
            let header = header_from_prev_blockhash(prev_hash);
            let hash = header.block_hash();
            let height = prev_height + 1;
            let connected_to = BlockId {
                height: prev_height,
                hash: prev_hash,
            };
            TestCase {
                name: "connected_to_prev_header_applied_to_self",
                chain: local_chain![(0, hash!("_")), (prev_height, prev_hash)],
                header,
                height,
                connected_to,
                exp_result: Ok(vec![(height, Some(hash))]),
            }
        },
        {
            let header = header_from_prev_blockhash(BlockHash::all_zeros());
            let hash = header.block_hash();
            let height = 0;
            let connected_to = BlockId { height, hash };
            TestCase {
                name: "genesis_applied_to_self",
                chain: local_chain![(0, hash)],
                header,
                height,
                connected_to,
                exp_result: Ok(vec![]),
            }
        },
        {
            let header = header_from_prev_blockhash(hash!("Z"));
            let height = 10;
            let hash = header.block_hash();
            let prev_height = height - 1;
            let prev_hash = header.prev_blockhash;
            TestCase {
                name: "connect_at_connected_to",
                chain: local_chain![(0, hash!("_")), (2, hash!("B")), (3, hash!("C"))],
                header,
                height: 10,
                connected_to: BlockId {
                    height: 3,
                    hash: hash!("C"),
                },
                exp_result: Ok(vec![(prev_height, Some(prev_hash)), (height, Some(hash))]),
            }
        },
        {
            let prev_hash = hash!("A");
            let prev_height = 1;
            let header = header_from_prev_blockhash(prev_hash);
            let connected_to = BlockId {
                height: prev_height,
                hash: hash!("not_prev_hash"),
            };
            TestCase {
                name: "inconsistent_prev_hash",
                chain: local_chain![(0, hash!("_")), (prev_height, hash!("not_prev_hash"))],
                header,
                height: prev_height + 1,
                connected_to,
                exp_result: Err(ApplyHeaderError::InconsistentBlocks),
            }
        },
        {
            let prev_hash = hash!("A");
            let prev_height = 1;
            let header = header_from_prev_blockhash(prev_hash);
            let height = prev_height + 1;
            let connected_to = BlockId {
                height,
                hash: hash!("not_current_hash"),
            };
            TestCase {
                name: "inconsistent_current_block",
                chain: local_chain![(0, hash!("_")), (height, hash!("not_current_hash"))],
                header,
                height,
                connected_to,
                exp_result: Err(ApplyHeaderError::InconsistentBlocks),
            }
        },
        {
            let header = header_from_prev_blockhash(hash!("B"));
            let height = 3;
            let connected_to = BlockId {
                height: 4,
                hash: hash!("D"),
            };
            TestCase {
                name: "connected_to_is_greater",
                chain: local_chain![(0, hash!("_")), (2, hash!("B"))],
                header,
                height,
                connected_to,
                exp_result: Err(ApplyHeaderError::InconsistentBlocks),
            }
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        let mut chain = t.chain;
        let result = chain.apply_header_connected_to(&t.header, t.height, t.connected_to);
        let exp_result = t
            .exp_result
            .map(|cs| cs.iter().cloned().collect::<ChangeSet>());
        assert_eq!(result, exp_result, "[{}:{}] unexpected result", i, t.name);
    }
}

fn generate_height_range_bounds(
    height_upper_bound: u32,
) -> impl Strategy<Value = (Bound<u32>, Bound<u32>)> {
    fn generate_height_bound(height_upper_bound: u32) -> impl Strategy<Value = Bound<u32>> {
        prop_oneof![
            (0..height_upper_bound).prop_map(Bound::Included),
            (0..height_upper_bound).prop_map(Bound::Excluded),
            Just(Bound::Unbounded),
        ]
    }
    (
        generate_height_bound(height_upper_bound),
        generate_height_bound(height_upper_bound),
    )
}

fn generate_checkpoints(max_height: u32, max_count: usize) -> impl Strategy<Value = CheckPoint> {
    proptest::collection::btree_set(1..max_height, 0..max_count).prop_map(|mut heights| {
        heights.insert(0); // must have genesis
        CheckPoint::from_block_ids(heights.into_iter().map(|height| {
            let hash = bitcoin::hashes::Hash::hash(height.to_le_bytes().as_slice());
            BlockId { height, hash }
        }))
        .expect("blocks must be in order as it comes from btreeset")
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        ..Default::default()
    })]

    /// Ensure that [`CheckPoint::range`] returns the expected checkpoint heights by comparing it
    /// against a more primitive approach.
    #[test]
    fn checkpoint_range(
        range in generate_height_range_bounds(21_000),
        cp in generate_checkpoints(21_000, 2100)
    ) {
        let exp_heights = cp.iter().map(|cp| cp.height()).filter(|h| range.contains(h)).collect::<Vec<u32>>();
        let heights = cp.range(range).map(|cp| cp.height()).collect::<Vec<u32>>();
        prop_assert_eq!(heights, exp_heights);
    }
}
