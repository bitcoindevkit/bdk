use bdk_core::{CheckPoint, ToBlockHash};
use bdk_testenv::hash;
use bitcoin::BlockHash;

// Custom struct for testing with prev_blockhash
#[derive(Debug, Clone, Copy)]
struct TestBlock {
    blockhash: BlockHash,
    prev_blockhash: BlockHash,
}

impl ToBlockHash for TestBlock {
    fn to_blockhash(&self) -> BlockHash {
        self.blockhash
    }

    fn prev_blockhash(&self) -> Option<BlockHash> {
        Some(self.prev_blockhash)
    }
}

/// Test that inserting at a new height with conflicting prev_blockhash displaces the checkpoint
/// below and purges all checkpoints above.
#[test]
fn checkpoint_insert_new_height_displaces_and_purges() {
    // Create chain: 98 -> 99 -> 100 -> 102 -> 103 (with gap at 101)
    let block_98 = TestBlock {
        blockhash: hash!("block_98"),
        prev_blockhash: hash!("block_97"),
    };
    let block_99 = TestBlock {
        blockhash: hash!("block_99"),
        prev_blockhash: hash!("block_98"),
    };
    let block_100 = TestBlock {
        blockhash: hash!("block_100"),
        prev_blockhash: hash!("block_99"),
    };
    let block_102 = TestBlock {
        blockhash: hash!("block_102"),
        prev_blockhash: hash!("block_101"), // References non-existent 101
    };
    let block_103 = TestBlock {
        blockhash: hash!("block_103"),
        prev_blockhash: hash!("block_102"),
    };

    let cp = CheckPoint::from_blocks(vec![
        (98, block_98),
        (99, block_99),
        (100, block_100),
        (102, block_102),
        (103, block_103),
    ])
    .expect("should create valid chain");

    // Insert a new block_101 that conflicts with `cp` at heights 100 and 101
    let block_101_conflicting = TestBlock {
        blockhash: hash!("block_101_new"),
        prev_blockhash: hash!("different_block_100"),
    };

    let result = cp.insert(101, block_101_conflicting);

    // Verify checkpoint 100 was displaced (omitted).
    assert!(result.get(100).is_none(), "`block_100` was displaced");

    // Verify checkpoints 102 and 103 were purged (orphaned)
    assert!(
        result.get(102).is_none(),
        "checkpoint at 102 should be purged"
    );
    assert!(
        result.get(103).is_none(),
        "checkpoint at 103 should be purged"
    );

    // Verify the tip is at 101
    assert_eq!(result.height(), 101);
    assert_eq!(result.hash(), hash!("block_101_new"));
}

/// Test that inserting at an existing height with conflicting prev_blockhash displaces the
/// checkpoint below and purges the original checkpoint and all above.
#[test]
fn checkpoint_insert_existing_height_with_prev_conflict() {
    // Create chain: 98 -> 99 -> 100 -> 101 -> 102
    let block_98 = TestBlock {
        blockhash: hash!("block_98"),
        prev_blockhash: hash!("block_97"),
    };
    let block_99 = TestBlock {
        blockhash: hash!("block_99"),
        prev_blockhash: hash!("block_98"),
    };
    let block_100 = TestBlock {
        blockhash: hash!("block_100"),
        prev_blockhash: hash!("block_99"),
    };
    let block_101 = TestBlock {
        blockhash: hash!("block_101"),
        prev_blockhash: hash!("block_100"),
    };
    let block_102 = TestBlock {
        blockhash: hash!("block_102"),
        prev_blockhash: hash!("block_101"),
    };

    let cp = CheckPoint::from_blocks(vec![
        (98, block_98),
        (99, block_99),
        (100, block_100),
        (101, block_101),
        (102, block_102),
    ])
    .expect("should create valid chain");

    // Insert at existing height 100 with prev_blockhash that conflicts with block 99
    let block_100_conflicting = TestBlock {
        blockhash: hash!("block_100_new"),
        prev_blockhash: hash!("different_block_99"),
    };

    let result = cp.insert(100, block_100_conflicting);

    // Verify checkpoint 99 was displaced to a placeholder
    assert!(result.get(99).is_none(), "`block_99` was displaced");

    // Verify checkpoints 101 and 102 were purged
    assert!(
        result.get(101).is_none(),
        "checkpoint at 101 should be purged"
    );
    assert!(
        result.get(102).is_none(),
        "checkpoint at 102 should be purged"
    );

    // Verify the tip is at 100
    assert_eq!(result.height(), 100);
    assert_eq!(result.hash(), hash!("block_100_new"));
}

/// Test that inserting at a new height without prev_blockhash conflict preserves the chain.
#[test]
fn checkpoint_insert_new_height_no_conflict() {
    // Use BlockHash which has no prev_blockhash
    let cp: CheckPoint<BlockHash> = CheckPoint::from_blocks(vec![
        (98, hash!("block_98")),
        (99, hash!("block_99")),
        (100, hash!("block_100")),
    ])
    .expect("should create valid chain");

    // Insert at new height 101 (no prev_blockhash to conflict)
    let result = cp.insert(101, hash!("block_101"));

    // All original checkpoints should remain unchanged
    assert_eq!(
        result.get(100).expect("checkpoint at 100").hash(),
        hash!("block_100")
    );

    assert_eq!(
        result.get(99).expect("checkpoint at 99").hash(),
        hash!("block_99")
    );

    assert_eq!(
        result.get(98).expect("checkpoint at 99").hash(),
        hash!("block_98")
    );

    // New checkpoint should be added
    assert_eq!(result.height(), 101);
    assert_eq!(result.hash(), hash!("block_101"));
    assert_eq!(
        result.iter().count(),
        4,
        "all checkpoints should be present (98, 99, 100, 101)"
    );
}

/// Test inserting a new root (99) into an existing chain (100, 101) results in a checkpoint
/// with all heights present (99, 100, 101)
#[test]
fn checkpoint_insert_new_root_connects_to_chain() {
    // Create chain: 100 -> 101 (missing root at 99)
    let block_100 = TestBlock {
        blockhash: hash!("block_100"),
        prev_blockhash: hash!("block_99"),
    };
    let block_101 = TestBlock {
        blockhash: hash!("block_101"),
        prev_blockhash: hash!("block_100"),
    };

    let cp = CheckPoint::from_blocks(vec![(100, block_100), (101, block_101)])
        .expect("should create valid chain");

    // Insert a new root block_99 that connects to block_100
    let block_99 = TestBlock {
        blockhash: hash!("block_99"), // Must match block_100.prev_blockhash
        prev_blockhash: hash!("block_98"),
    };

    let result = cp.insert(99, block_99);

    // Verify all heights are present (99, 100, 101)
    assert_eq!(
        result.iter().count(),
        3,
        "should have 3 checkpoints (99, 100, 101)"
    );

    // Verify height 99 was inserted correctly
    let height_99 = result.get(99).expect("checkpoint at 99 should exist");
    assert_eq!(height_99.hash(), hash!("block_99"));

    // Verify existing checkpoints remain unchanged
    let height_100 = result.get(100).expect("checkpoint at 100 should exist");
    assert_eq!(height_100.hash(), hash!("block_100"));

    let height_101 = result.get(101).expect("checkpoint at 101 should exist");
    assert_eq!(height_101.hash(), hash!("block_101"));

    // Verify the tip is still at 101
    assert_eq!(result.height(), 101);
    assert_eq!(result.hash(), hash!("block_101"));
}

/// Test that displacement of the root (by omission) of an original chain (100, 102),
/// results in a new checkpoint with a new root and remaining tail (101_new, 102),
/// assuming the tail connects.
#[test]
fn checkpoint_displace_root_greater_than_zero() {
    // Create chain: 100 -> 101 -> 102
    let block_100 = TestBlock {
        blockhash: hash!("block_100"),
        prev_blockhash: hash!("block_99"),
    };
    let block_102 = TestBlock {
        blockhash: hash!("block_102"),
        prev_blockhash: hash!("block_101"),
    };

    let cp = CheckPoint::from_blocks(vec![(100, block_100), (102, block_102)])
        .expect("should create valid chain");

    // Insert at height 101 with prev_blockhash that conflicts with block_100
    let block_101_new = TestBlock {
        blockhash: hash!("block_101"),
        prev_blockhash: hash!("different_block_100"), // Conflicts with block_100.hash
    };

    let result = cp.insert(101, block_101_new);

    // Verify checkpoint 100 was displaced (omitted)
    assert!(
        result.get(100).is_none(),
        "checkpoint at 100 should be displaced"
    );

    // Verify checkpoint 101 has the new block
    let checkpoint_101 = result.get(101).expect("checkpoint at 101 should exist");
    assert_eq!(checkpoint_101.hash(), block_101_new.blockhash);

    // Verify checkpoint 102 remains (it references the new block_101)
    let checkpoint_102 = result.get(102).expect("checkpoint at 102 should exist");
    assert_eq!(checkpoint_102.hash(), hash!("block_102"));

    // Verify the result has 2 checkpoints (101_new, 102)
    assert_eq!(
        result.iter().count(),
        2,
        "should have 2 checkpoints (101_new, 102)"
    );

    // Verify the tip is at 102
    assert_eq!(result.height(), 102);
    assert_eq!(result.hash(), hash!("block_102"));
}

// Test `insert` displaces the root of a single-element chain (block_100.hash not equal
// block_101.prev_blockhash)
#[test]
fn checkpoint_displace_root_single_node_chain() {
    // Create chain: 100
    let block_100 = TestBlock {
        blockhash: hash!("block_100"),
        prev_blockhash: hash!("block_99"),
    };

    let cp = CheckPoint::from_blocks(vec![(100, block_100)]).expect("should create valid chain");

    // Insert at height 101 with prev_blockhash that conflicts with block_100
    let block_101_new = TestBlock {
        blockhash: hash!("block_101"),
        prev_blockhash: hash!("different_block_100"), // Conflicts with block_100.hash
    };

    let result = cp.insert(101, block_101_new);

    // Verify checkpoint 100 was displaced (omitted)
    assert!(
        result.get(100).is_none(),
        "checkpoint at 100 should be displaced"
    );

    // Verify the result has 2 checkpoints (101_new, 102)
    assert_eq!(
        result.iter().count(),
        1,
        "should have 1 checkpoint(s) (101_new)"
    );

    // Verify the tip is at 101
    assert_eq!(result.height(), 101);
    assert_eq!(result.hash(), block_101_new.blockhash);
}

/// Test `insert` should panic if trying to replace the node at height 0
#[test]
#[should_panic(expected = "cannot replace the genesis block")]
fn checkpoint_insert_cannot_replace_genesis() {
    // Create chain with genesis at height 0
    let block_0 = TestBlock {
        blockhash: hash!("block_0"),
        prev_blockhash: hash!("genesis_parent"),
    };
    let block_1 = TestBlock {
        blockhash: hash!("block_1"),
        prev_blockhash: hash!("block_0"),
    };

    let cp = CheckPoint::from_blocks(vec![(0, block_0), (1, block_1)])
        .expect("should create valid chain");

    // Try to insert at height 1 with prev_blockhash that conflicts with block_0
    let block_0_new = TestBlock {
        blockhash: hash!("block_0_new"),
        prev_blockhash: hash!("genesis_parent_new"),
    };

    // This should panic because it would try to replace the genesis checkpoint at height 0
    let _ = cp.insert(0, block_0_new);
}

/// Test `insert` should panic if trying to displace (by omission) the node at height 0
#[test]
#[should_panic(expected = "cannot replace the genesis block")]
fn checkpoint_insert_cannot_displace_genesis() {
    // Create chain with genesis at height 0
    let block_0 = TestBlock {
        blockhash: hash!("block_0"),
        prev_blockhash: hash!("genesis_parent"),
    };
    let block_1 = TestBlock {
        blockhash: hash!("block_1"),
        prev_blockhash: hash!("block_0"),
    };

    let cp = CheckPoint::from_blocks(vec![(0, block_0), (1, block_1)])
        .expect("should create valid chain");

    // Try to insert at height 1 with prev_blockhash that conflicts with block_0
    let block_1_new = TestBlock {
        blockhash: hash!("block_1_new"),
        prev_blockhash: hash!("different_block_0"), // Conflicts with block_0.hash
    };

    // This should panic because it would try to displace the genesis checkpoint at height 0
    let _ = cp.insert(1, block_1_new);
}
