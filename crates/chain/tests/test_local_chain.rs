use bdk_chain::local_chain::{
    CannotConnectError, ChangeSet, CheckPoint, InsertBlockError, LocalChain,
};
use bitcoin::BlockHash;

#[macro_use]
mod common;

#[derive(Debug)]
struct TestLocalChain<'a> {
    name: &'static str,
    chain: LocalChain,
    new_tip: CheckPoint,
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

impl<'a> TestLocalChain<'a> {
    fn run(mut self) {
        let got_changeset = match self.chain.update(self.new_tip) {
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
            chain: local_chain![],
            new_tip: chain_update![(0, h!("A"))],
            exp: ExpectedResult::Ok {
                changeset: &[(0, Some(h!("A")))],
                init_changeset: &[(0, Some(h!("A")))],
            },
        },
        TestLocalChain {
            name: "add second tip",
            chain: local_chain![(0, h!("A"))],
            new_tip: chain_update![(0, h!("A")), (1, h!("B"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(h!("B")))],
                init_changeset: &[(0, Some(h!("A"))), (1, Some(h!("B")))],
            },
        },
        TestLocalChain {
            name: "two disjoint chains cannot merge",
            chain: local_chain![(0, h!("A"))],
            new_tip: chain_update![(1, h!("B"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 0,
            }),
        },
        TestLocalChain {
            name: "two disjoint chains cannot merge (existing chain longer)",
            chain: local_chain![(1, h!("A"))],
            new_tip: chain_update![(0, h!("B"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 1,
            }),
        },
        TestLocalChain {
            name: "duplicate chains should merge",
            chain: local_chain![(0, h!("A"))],
            new_tip: chain_update![(0, h!("A"))],
            exp: ExpectedResult::Ok {
                changeset: &[],
                init_changeset: &[(0, Some(h!("A")))],
            },
        },
        TestLocalChain {
            name: "can introduce older checkpoints",
            chain: local_chain![(2, h!("C")), (3, h!("D"))],
            new_tip: chain_update![(1, h!("B")), (2, h!("C"))],
            exp: ExpectedResult::Ok {
                changeset: &[(1, Some(h!("B")))],
                init_changeset: &[(1, Some(h!("B"))), (2, Some(h!("C"))), (3, Some(h!("D")))],
            },
        },
        TestLocalChain {
            name: "fix blockhash before agreement point",
            chain: local_chain![(0, h!("im-wrong")), (1, h!("we-agree"))],
            new_tip: chain_update![(0, h!("fix")), (1, h!("we-agree"))],
            exp: ExpectedResult::Ok {
                changeset: &[(0, Some(h!("fix")))],
                init_changeset: &[(0, Some(h!("fix"))), (1, Some(h!("we-agree")))],
            },
        },
        // B and C are in both chain and update
        //        | 0 | 1 | 2 | 3 | 4
        // chain  |     B   C
        // update | A   B   C   D
        // This should succeed with the point of agreement being C and A should be added in addition.
        TestLocalChain {
            name: "two points of agreement",
            chain: local_chain![(1, h!("B")), (2, h!("C"))],
            new_tip: chain_update![(0, h!("A")), (1, h!("B")), (2, h!("C")), (3, h!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[(0, Some(h!("A"))), (3, Some(h!("D")))],
                init_changeset: &[
                    (0, Some(h!("A"))),
                    (1, Some(h!("B"))),
                    (2, Some(h!("C"))),
                    (3, Some(h!("D"))),
                ],
            },
        },
        // Update and chain does not connect:
        //        | 0 | 1 | 2 | 3 | 4
        // chain  |     B   C
        // update | A   B       D
        // This should fail as we cannot figure out whether C & D are on the same chain
        TestLocalChain {
            name: "update and chain does not connect",
            chain: local_chain![(1, h!("B")), (2, h!("C"))],
            new_tip: chain_update![(0, h!("A")), (1, h!("B")), (3, h!("D"))],
            exp: ExpectedResult::Err(CannotConnectError {
                try_include_height: 2,
            }),
        },
        // Transient invalidation:
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | A       B   C       E
        // update | A       B'  C'  D
        // This should succeed and invalidate B,C and E with point of agreement being A.
        TestLocalChain {
            name: "transitive invalidation applies to checkpoints higher than invalidation",
            chain: local_chain![(0, h!("A")), (2, h!("B")), (3, h!("C")), (5, h!("E"))],
            new_tip: chain_update![(0, h!("A")), (2, h!("B'")), (3, h!("C'")), (4, h!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (2, Some(h!("B'"))),
                    (3, Some(h!("C'"))),
                    (4, Some(h!("D"))),
                    (5, None),
                ],
                init_changeset: &[
                    (0, Some(h!("A"))),
                    (2, Some(h!("B'"))),
                    (3, Some(h!("C'"))),
                    (4, Some(h!("D"))),
                ],
            },
        },
        // Transient invalidation:
        //        | 0 | 1 | 2 | 3 | 4
        // chain  |     B   C       E
        // update |     B'  C'  D
        // This should succeed and invalidate B, C and E with no point of agreement
        TestLocalChain {
            name: "transitive invalidation applies to checkpoints higher than invalidation no point of agreement",
            chain: local_chain![(1, h!("B")), (2, h!("C")), (4, h!("E"))],
            new_tip: chain_update![(1, h!("B'")), (2, h!("C'")), (3, h!("D"))],
            exp: ExpectedResult::Ok {
                changeset: &[
                    (1, Some(h!("B'"))),
                    (2, Some(h!("C'"))),
                    (3, Some(h!("D"))),
                    (4, None)
                ],
                init_changeset: &[
                    (1, Some(h!("B'"))),
                    (2, Some(h!("C'"))),
                    (3, Some(h!("D"))),
                ],
            },
        },
        // Transient invalidation:
        //        | 0 | 1 | 2 | 3 | 4
        // chain  | A   B   C       E
        // update |     B'  C'  D
        // This should fail since although it tells us that B and C are invalid it doesn't tell us whether
        // A was invalid.
        TestLocalChain {
            name: "invalidation but no connection",
            chain: local_chain![(0, h!("A")), (1, h!("B")), (2, h!("C")), (4, h!("E"))],
            new_tip: chain_update![(1, h!("B'")), (2, h!("C'")), (3, h!("D"))],
            exp: ExpectedResult::Err(CannotConnectError { try_include_height: 0 }),
        },
        // Introduce blocks between two points of agreement
        //        | 0 | 1 | 2 | 3 | 4 | 5
        // chain  | A   B       D   E
        // update | A       C       E   F
        TestLocalChain {
            name: "introduce blocks between two points of agreement",
            chain: local_chain![(0, h!("A")), (1, h!("B")), (3, h!("D")), (4, h!("E"))],
            new_tip: chain_update![(0, h!("A")), (2, h!("C")), (4, h!("E")), (5, h!("F"))],
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
    ]
    .into_iter()
    .for_each(TestLocalChain::run);
}

#[test]
fn local_chain_insert_block() {
    struct TestCase {
        original: LocalChain,
        insert: (u32, BlockHash),
        expected_result: Result<ChangeSet, InsertBlockError>,
        expected_final: LocalChain,
    }

    let test_cases = [
        TestCase {
            original: local_chain![],
            insert: (5, h!("block5")),
            expected_result: Ok([(5, Some(h!("block5")))].into()),
            expected_final: local_chain![(5, h!("block5"))],
        },
        TestCase {
            original: local_chain![(3, h!("A"))],
            insert: (4, h!("B")),
            expected_result: Ok([(4, Some(h!("B")))].into()),
            expected_final: local_chain![(3, h!("A")), (4, h!("B"))],
        },
        TestCase {
            original: local_chain![(4, h!("B"))],
            insert: (3, h!("A")),
            expected_result: Ok([(3, Some(h!("A")))].into()),
            expected_final: local_chain![(3, h!("A")), (4, h!("B"))],
        },
        TestCase {
            original: local_chain![(2, h!("K"))],
            insert: (2, h!("K")),
            expected_result: Ok([].into()),
            expected_final: local_chain![(2, h!("K"))],
        },
        TestCase {
            original: local_chain![(2, h!("K"))],
            insert: (2, h!("J")),
            expected_result: Err(InsertBlockError {
                height: 2,
                original_hash: h!("K"),
                update_hash: h!("J"),
            }),
            expected_final: local_chain![(2, h!("K"))],
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
