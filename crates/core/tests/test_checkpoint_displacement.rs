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

    // Insert at height 101 with prev_blockhash that conflicts with block 100
    let block_101_conflicting = TestBlock {
        blockhash: hash!("block_101_new"),
        prev_blockhash: hash!("different_block_100"),
    };

    let result = cp.insert(101, block_101_conflicting);

    // Verify checkpoint 100 was displaced to a placeholder
    let cp_100 = result.get(100).expect("checkpoint at 100 should exist");
    assert_eq!(cp_100.hash(), hash!("different_block_100"));
    assert!(
        cp_100.data_ref().is_none(),
        "checkpoint at 100 should be a placeholder"
    );

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
    let cp_99 = result.get(99).expect("checkpoint at 99 should exist");
    assert_eq!(cp_99.hash(), hash!("different_block_99"));
    assert!(
        cp_99.data_ref().is_none(),
        "checkpoint at 99 should be a placeholder"
    );

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
    assert!(result.get(100).unwrap().data_ref().is_some());

    assert_eq!(
        result.get(99).expect("checkpoint at 99").hash(),
        hash!("block_99")
    );
    assert!(result.get(99).unwrap().data_ref().is_some());

    // New checkpoint should be added
    assert_eq!(result.height(), 101);
    assert_eq!(result.hash(), hash!("block_101"));
}
