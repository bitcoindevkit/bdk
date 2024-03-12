use bdk_chain::{
    local_chain::{
        AlterCheckPointError, ApplyHeaderError, CannotConnectError, ChangeSet, CheckPoint,
        LocalChain, MissingGenesisError, Update,
    },
    BlockId,
};
use bitcoin::{block::Header, hashes::Hash, BlockHash};

#[macro_use]
mod common;

#[derive(Debug)]
struct TestLocalChain<'a> {
    name: &'static str,
    chain: LocalChain,
    update: Update,
    exp: ExpectedResult<'a>,
}

#[derive(Debug, PartialEq, Eq)]
enum ExpectedResult<'a> {
    Ok {
        changeset: &'a [(u32, Option<BlockHash>)],
        init_changeset: &'a [(u32, Option<BlockHash>)],
    },
    Err(CannotConnectError),
}

impl<'a> TestLocalChain<'a> {
    fn run(mut self) {
        println!("[TestLocalChain] test: {}", self.name);
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
            chain: local_chain![(0, h!("A"))],
            update: chain_update![(0, h!("A"))],
            exp: ExpectedResult::Ok {
                changeset: &[],
                init_changeset: &[(0, Some(h!("A")))],
            },
        },
        TestLocalChain {
            name: "add second tip",
            chain: local_chain![(0, h!("A"))],
            update: chain_update![(0, h!("A")), (1, h!("B"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(h!("B")))],
                init_changeset: &[(0, Some(h!("A"))), (1, Some(h!("B")))],
            },
        },
        TestLocalChain {
            name: "two disjoint chains cannot merge",
            chain: local_chain![(0, h!("_")), (1, h!("A"))],
            update: chain_update![(0, h!("_")), (2, h!("B"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 1,
            }),
        },
        TestLocalChain {
            name: "two disjoint chains cannot merge (existing chain longer)",
            chain: local_chain![(0, h!("_")), (2, h!("A"))],
            update: chain_update![(0, h!("_")), (1, h!("B"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 2,
            }),
        },
        TestLocalChain {
            name: "duplicate chains should merge",
            chain: local_chain![(0, h!("A"))],
            update: chain_update![(0, h!("A"))],
            exp: ExpectedResult::Ok {
                changeset: &[],
                init_changeset: &[(0, Some(h!("A")))],
            },
        },
        // Introduce an older checkpoint (B)
        //        | 0 | 1 | 2 | 3
        // chain  | _       C   D
        // update | _   B   C
        TestLocalChain {
            name: "can introduce older checkpoint",
            chain: local_chain![(0, h!("_")), (2, h!("C")), (3, h!("D"))],
            update: chain_update![(0, h!("_")), (1, h!("B")), (2, h!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(h!("B")))],
                init_changeset: &[(0, Some(h!("_"))), (1, Some(h!("B"))), (2, Some(h!("C"))), (3, Some(h!("D")))],
            },
        },
        // Introduce an older checkpoint (A) that is not directly behind PoA
        //        | 0 | 2 | 3 | 4
        // chain  | _       B   C
        // update | _   A       C
        TestLocalChain {
            name: "can introduce older checkpoint 2",
            chain: local_chain![(0, h!("_")), (3, h!("B")), (4, h!("C"))],
            update: chain_update![(0, h!("_")), (2, h!("A")), (4, h!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(2, Some(h!("A")))],
                init_changeset: &[(0, Some(h!("_"))), (2, Some(h!("A"))), (3, Some(h!("B"))), (4, Some(h!("C")))],
            }
        },
        // Introduce an older checkpoint (B) that is not the oldest checkpoint
        //        | 0 | 1 | 2 | 3
        // chain  | _   A       C
        // update | _       B   C
        TestLocalChain {
            name: "can introduce older checkpoint 3",
            chain: local_chain![(0, h!("_")), (1, h!("A")), (3, h!("C"))],
            update: chain_update![(0, h!("_")), (2, h!("B")), (3, h!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(2, Some(h!("B")))],
                init_changeset: &[(0, Some(h!("_"))), (1, Some(h!("A"))), (2, Some(h!("B"))), (3, Some(h!("C")))],
            }
        },
        // Introduce two older checkpoints below the PoA
        //        | 0 | 1 | 2 | 3
        // chain  | _           C
        // update | _   A   B   C
        TestLocalChain {
            name: "introduce two older checkpoints below PoA",
            chain: local_chain![(0, h!("_")), (3, h!("C"))],
            update: chain_update![(0, h!("_")), (1, h!("A")), (2, h!("B")), (3, h!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(h!("A"))), (2, Some(h!("B")))],
                init_changeset: &[(0, Some(h!("_"))), (1, Some(h!("A"))), (2, Some(h!("B"))), (3, Some(h!("C")))],
            },
        },
        TestLocalChain {
            name: "fix blockhash before agreement point",
            chain: local_chain![(0, h!("im-wrong")), (1, h!("we-agree"))],
            update: chain_update![(0, h!("fix")), (1, h!("we-agree"))],
            exp: ExpectedResult::Ok {
                changeset: &[(0, Some(h!("fix")))],
                init_changeset: &[(0, Some(h!("fix"))), (1, Some(h!("we-agree")))],
            },
        },
        // B and C are in both chain and update
        //        | 0 | 1 | 2 | 3 | 4
        // chain  | _       B   C
        // update | _   A   B   C   D
        // This should succeed with the point of agreement being C and A should be added in addition.
        TestLocalChain {
            name: "two points of agreement",
            chain: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C"))],
            update: chain_update![(0, h!("_")), (1, h!("A")), (2, h!("B")), (3, h!("C")), (4, h!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(h!("A"))), (4, Some(h!("D")))],
                init_changeset: &[
                    (0, Some(h!("_"))),
                    (1, Some(h!("A"))),
                    (2, Some(h!("B"))),
                    (3, Some(h!("C"))),
                    (4, Some(h!("D"))),
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
            chain: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C"))],
            update: chain_update![(0, h!("_")), (1, h!("A")), (2, h!("B")), (4, h!("D"))],
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
            chain: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C")), (5, h!("E"))],
            update: chain_update![(0, h!("_")), (2, h!("B'")), (3, h!("C'")), (4, h!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (2, Some(h!("B'"))),
                    (3, Some(h!("C'"))),
                    (4, Some(h!("D"))),
                    (5, None),
                ],
                init_changeset: &[
                    (0, Some(h!("_"))),
                    (2, Some(h!("B'"))),
                    (3, Some(h!("C'"))),
                    (4, Some(h!("D"))),
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
            chain: local_chain![(0, h!("_")), (1, h!("B")), (2, h!("C")), (4, h!("E"))],
            update: chain_update![(0, h!("_")), (1, h!("B'")), (2, h!("C'")), (3, h!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (1, Some(h!("B'"))),
                    (2, Some(h!("C'"))),
                    (3, Some(h!("D"))),
                    (4, None)
                ],
                init_changeset: &[
                    (0, Some(h!("_"))), 
                    (1, Some(h!("B'"))),
                    (2, Some(h!("C'"))),
                    (3, Some(h!("D"))),
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
            chain: local_chain![(0, h!("_")), (1, h!("A")), (2, h!("B")), (3, h!("C")), (5, h!("E"))],
            update: chain_update![(0, h!("_")), (2, h!("B'")), (3, h!("C'")), (4, h!("D"))],
            exp: ExpectedResult::Err(CannotConnectError { try_include_height: 1 }),
        },
        // Introduce blocks between two points of agreement
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | A   B       D   E
        // update | A       C       E   F
        TestLocalChain {
            name: "introduce blocks between two points of agreement",
            chain: local_chain![(0, h!("A")), (1, h!("B")), (3, h!("D")), (4, h!("E"))],
            update: chain_update![(0, h!("A")), (2, h!("C")), (4, h!("E")), (5, h!("F"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (2, Some(h!("C"))),
                    (5, Some(h!("F"))),
                ],
                init_changeset: &[
                    (0, Some(h!("A"))),
                    (1, Some(h!("B"))),
                    (2, Some(h!("C"))),
                    (3, Some(h!("D"))),
                    (4, Some(h!("E"))),
                    (5, Some(h!("F"))),
                ],
            },
        },
        // Allow update that is shorter than original chain
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | A       C   D   E   F
        // update | A       C   D'
        TestLocalChain {
            name: "allow update that is shorter than original chain",
            chain: local_chain![(0, h!("_")), (2, h!("C")), (3, h!("D")), (4, h!("E")), (5, h!("F"))],
            update: chain_update![(0, h!("_")), (2, h!("C")), (3, h!("D'"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (3, Some(h!("D'"))),
                    (4, None),
                    (5, None),
                ],
                init_changeset: &[
                    (0, Some(h!("_"))),
                    (2, Some(h!("C"))),
                    (3, Some(h!("D'"))),
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
            original: local_chain![(0, h!("_"))],
            insert: (5, h!("block5")),
            expected_result: Ok([(5, Some(h!("block5")))].into()),
            expected_final: local_chain![(0, h!("_")), (5, h!("block5"))],
        },
        TestCase {
            original: local_chain![(0, h!("_")), (3, h!("A"))],
            insert: (4, h!("B")),
            expected_result: Ok([(4, Some(h!("B")))].into()),
            expected_final: local_chain![(0, h!("_")), (3, h!("A")), (4, h!("B"))],
        },
        TestCase {
            original: local_chain![(0, h!("_")), (4, h!("B"))],
            insert: (3, h!("A")),
            expected_result: Ok([(3, Some(h!("A")))].into()),
            expected_final: local_chain![(0, h!("_")), (3, h!("A")), (4, h!("B"))],
        },
        TestCase {
            original: local_chain![(0, h!("_")), (2, h!("K"))],
            insert: (2, h!("K")),
            expected_result: Ok([].into()),
            expected_final: local_chain![(0, h!("_")), (2, h!("K"))],
        },
        TestCase {
            original: local_chain![(0, h!("_")), (2, h!("K"))],
            insert: (2, h!("J")),
            expected_result: Err(AlterCheckPointError {
                height: 2,
                original_hash: h!("K"),
                update_hash: Some(h!("J")),
            }),
            expected_final: local_chain![(0, h!("_")), (2, h!("K"))],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        let mut chain = t.original;
        assert_eq!(
            chain.insert_block(t.insert.into()),
            t.expected_result,
            "[{}] unexpected result when inserting block",
            i,
        );
        assert_eq!(chain, t.expected_final, "[{}] unexpected final chain", i,);
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
            original: local_chain![(0, h!("_"))],
            disconnect_from: (0, h!("_")),
            exp_result: Err(MissingGenesisError),
            exp_final: local_chain![(0, h!("_"))],
        },
        TestCase {
            name: "try_replace_genesis_should_fail_2",
            original: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C"))],
            disconnect_from: (0, h!("_")),
            exp_result: Err(MissingGenesisError),
            exp_final: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C"))],
        },
        TestCase {
            name: "from_does_not_exist",
            original: local_chain![(0, h!("_")), (3, h!("C"))],
            disconnect_from: (2, h!("B")),
            exp_result: Ok(ChangeSet::default()),
            exp_final: local_chain![(0, h!("_")), (3, h!("C"))],
        },
        TestCase {
            name: "from_has_different_blockhash",
            original: local_chain![(0, h!("_")), (2, h!("B"))],
            disconnect_from: (2, h!("not_B")),
            exp_result: Ok(ChangeSet::default()),
            exp_final: local_chain![(0, h!("_")), (2, h!("B"))],
        },
        TestCase {
            name: "disconnect_one",
            original: local_chain![(0, h!("_")), (2, h!("B"))],
            disconnect_from: (2, h!("B")),
            exp_result: Ok(ChangeSet::from_iter([(2, None)])),
            exp_final: local_chain![(0, h!("_"))],
        },
        TestCase {
            name: "disconnect_three",
            original: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C")), (4, h!("D"))],
            disconnect_from: (2, h!("B")),
            exp_result: Ok(ChangeSet::from_iter([(2, None), (3, None), (4, None)])),
            exp_final: local_chain![(0, h!("_"))],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        println!("Case {}: {}", i, t.name);

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
            blocks: &[(0, h!("A")), (1, h!("B")), (3, h!("D"))],
            exp_result: Ok(()),
        },
        TestCase {
            name: "with_duplicates",
            blocks: &[(1, h!("B")), (2, h!("C")), (2, h!("C'"))],
            exp_result: Err(Some((2, h!("C")))),
        },
        TestCase {
            name: "not_in_order",
            blocks: &[(1, h!("B")), (3, h!("D")), (2, h!("C"))],
            exp_result: Err(Some((3, h!("D")))),
        },
        TestCase {
            name: "empty",
            blocks: &[],
            exp_result: Err(None),
        },
        TestCase {
            name: "single",
            blocks: &[(21, h!("million"))],
            exp_result: Ok(()),
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        println!("running test case {}: '{}'", i, t.name);
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
            let header = header_from_prev_blockhash(h!("A"));
            let hash = header.block_hash();
            let height = 2;
            let connected_to = BlockId { height, hash };
            TestCase {
                name: "connected_to_self_header_applied_to_self",
                chain: local_chain![(0, h!("_")), (height, hash)],
                header,
                height,
                connected_to,
                exp_result: Ok(vec![]),
            }
        },
        {
            let prev_hash = h!("A");
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
                chain: local_chain![(0, h!("_")), (prev_height, prev_hash)],
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
            let header = header_from_prev_blockhash(h!("Z"));
            let height = 10;
            let hash = header.block_hash();
            let prev_height = height - 1;
            let prev_hash = header.prev_blockhash;
            TestCase {
                name: "connect_at_connected_to",
                chain: local_chain![(0, h!("_")), (2, h!("B")), (3, h!("C"))],
                header,
                height: 10,
                connected_to: BlockId {
                    height: 3,
                    hash: h!("C"),
                },
                exp_result: Ok(vec![(prev_height, Some(prev_hash)), (height, Some(hash))]),
            }
        },
        {
            let prev_hash = h!("A");
            let prev_height = 1;
            let header = header_from_prev_blockhash(prev_hash);
            let connected_to = BlockId {
                height: prev_height,
                hash: h!("not_prev_hash"),
            };
            TestCase {
                name: "inconsistent_prev_hash",
                chain: local_chain![(0, h!("_")), (prev_height, h!("not_prev_hash"))],
                header,
                height: prev_height + 1,
                connected_to,
                exp_result: Err(ApplyHeaderError::InconsistentBlocks),
            }
        },
        {
            let prev_hash = h!("A");
            let prev_height = 1;
            let header = header_from_prev_blockhash(prev_hash);
            let height = prev_height + 1;
            let connected_to = BlockId {
                height,
                hash: h!("not_current_hash"),
            };
            TestCase {
                name: "inconsistent_current_block",
                chain: local_chain![(0, h!("_")), (height, h!("not_current_hash"))],
                header,
                height,
                connected_to,
                exp_result: Err(ApplyHeaderError::InconsistentBlocks),
            }
        },
        {
            let header = header_from_prev_blockhash(h!("B"));
            let height = 3;
            let connected_to = BlockId {
                height: 4,
                hash: h!("D"),
            };
            TestCase {
                name: "connected_to_is_greater",
                chain: local_chain![(0, h!("_")), (2, h!("B"))],
                header,
                height,
                connected_to,
                exp_result: Err(ApplyHeaderError::InconsistentBlocks),
            }
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        println!("running test case {}: '{}'", i, t.name);
        let mut chain = t.chain;
        let result = chain.apply_header_connected_to(&t.header, t.height, t.connected_to);
        let exp_result = t
            .exp_result
            .map(|cs| cs.iter().cloned().collect::<ChangeSet>());
        assert_eq!(result, exp_result, "[{}:{}] unexpected result", i, t.name);
    }
}
