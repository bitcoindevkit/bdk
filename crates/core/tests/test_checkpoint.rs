use bdk_core::{CheckPoint, ToBlockHash};
use bdk_testenv::{block_id, hash};
use bitcoin::BlockHash;

/// Inserting a block that already exists in the checkpoint chain must always succeed.
#[test]
fn checkpoint_insert_existing() {
    let blocks = &[
        block_id!(0, "genesis"),
        block_id!(1, "A"),
        block_id!(2, "B"),
        block_id!(3, "C"),
    ];

    // Index `i` allows us to test with chains of different length.
    // Index `j` allows us to test inserting different block heights.
    for i in 0..blocks.len() {
        let cp_chain = CheckPoint::from_blocks(
            blocks[..=i]
                .iter()
                .copied()
                .map(|block_id| (block_id.height, block_id.hash)),
        )
        .expect("must construct valid chain");

        for j in 0..=i {
            let block_to_insert = cp_chain
                .get(j as u32)
                .expect("cp of height must exist")
                .block_id();
            let new_cp_chain = cp_chain
                .clone()
                .insert(block_to_insert.height, block_to_insert.hash);

            assert_eq!(
                new_cp_chain, cp_chain,
                "must not divert from original chain"
            );
            assert!(new_cp_chain.eq_ptr(&cp_chain), "pointers must still match");
        }
    }
}

#[test]
fn checkpoint_destruction_is_sound() {
    // Prior to the fix in https://github.com/bitcoindevkit/bdk/pull/1731
    // this could have caused a stack overflow due to drop recursion in Arc.
    // We test that a long linked list can clean itself up without blowing
    // out the stack.
    let mut cp = CheckPoint::new(0, hash!("g"));
    let end = 10_000;
    for height in 1u32..end {
        let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
        cp = cp.push(height, hash).unwrap();
    }
    assert_eq!(cp.iter().count() as u32, end);
}

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

/// Test inserting data with conflicting prev_blockhash should displace checkpoint and create
/// placeholder.
///
/// When inserting data at height `h` with a `prev_blockhash` that conflicts with the checkpoint
/// at height `h-1`, the checkpoint at `h-1` should be displaced and replaced with a placeholder
/// containing the `prev_blockhash` from the inserted data.
///
/// Expected: Checkpoint at 99 gets displaced when inserting at 100 with conflicting prev_blockhash.
#[test]
fn checkpoint_insert_conflicting_prev_blockhash() {
    // Create initial checkpoint at height 99
    let block_99 = TestBlock {
        blockhash: hash!("block_at_99"),
        prev_blockhash: hash!("block_at_98"),
    };
    let cp = CheckPoint::new(99, block_99);

    // The initial chain has a placeholder at 98 (from block_99's prev_blockhash)
    assert_eq!(cp.iter().count(), 2);
    let height_98_before = cp.get(98).expect("should have checkpoint at 98");
    assert_eq!(height_98_before.hash(), block_99.prev_blockhash);
    assert!(
        height_98_before.data_ref().is_none(),
        "98 should be placeholder initially"
    );

    // Insert data at height 100 with a prev_blockhash that conflicts with checkpoint at 99
    let block_100_conflicting = TestBlock {
        blockhash: hash!("block_at_100"),
        prev_blockhash: hash!("different_block_at_99"), // Conflicts with block_99.blockhash
    };

    let result = cp.insert(100, block_100_conflicting);

    // Expected behavior: The checkpoint at 99 should be displaced and replaced with a placeholder
    let height_99_after = result.get(99).expect("checkpoint at 99 should exist");
    assert_eq!(
        height_99_after.hash(),
        block_100_conflicting.prev_blockhash,
        "checkpoint at 99 should be displaced and have the prev_blockhash from inserted data"
    );
    assert!(
        height_99_after.data_ref().is_none(),
        "checkpoint at 99 should be a placeholder (no data) after displacement"
    );

    // The checkpoint at 100 should be inserted correctly
    let height_100 = result.get(100).expect("checkpoint at 100 should exist");
    assert_eq!(height_100.hash(), block_100_conflicting.blockhash);
    assert!(
        height_100.data_ref().is_some(),
        "checkpoint at 100 should have data"
    );

    // Verify chain structure
    assert_eq!(result.height(), 100, "tip should be at height 100");
    assert_eq!(
        result.iter().count(),
        3,
        "should have 3 checkpoints (98 placeholder, 99 placeholder, 100)"
    );
}

/// Test inserting data that conflicts with prev_blockhash of higher checkpoints should purge them.
///
/// When inserting data at height `h` where the blockhash conflicts with the `prev_blockhash` of
/// checkpoint at height `h+1`, the checkpoint at `h+1` and all checkpoints above it should be
/// purged from the chain.
///
/// Expected: Checkpoints at 100, 101, 102 get purged when inserting at 99 with conflicting
/// blockhash.
#[test]
fn checkpoint_insert_purges_conflicting_tail() {
    // Create a chain with multiple checkpoints
    let block_98 = TestBlock {
        blockhash: hash!("block_at_98"),
        prev_blockhash: hash!("block_at_97"),
    };
    let block_99 = TestBlock {
        blockhash: hash!("block_at_99"),
        prev_blockhash: hash!("block_at_98"),
    };
    let block_100 = TestBlock {
        blockhash: hash!("block_at_100"),
        prev_blockhash: hash!("block_at_99"),
    };
    let block_101 = TestBlock {
        blockhash: hash!("block_at_101"),
        prev_blockhash: hash!("block_at_100"),
    };
    let block_102 = TestBlock {
        blockhash: hash!("block_at_102"),
        prev_blockhash: hash!("block_at_101"),
    };

    let cp = CheckPoint::from_blocks(vec![
        (98, block_98),
        (99, block_99),
        (100, block_100),
        (101, block_101),
        (102, block_102),
    ])
    .expect("should create valid checkpoint chain");

    // Verify initial chain has all checkpoints
    assert_eq!(cp.iter().count(), 6); // 97 (placeholder), 98, 99, 100, 101, 102

    // Insert a conflicting block at height 99
    // The new block's hash will conflict with block_100's prev_blockhash
    let conflicting_block_99 = TestBlock {
        blockhash: hash!("different_block_at_99"),
        prev_blockhash: hash!("block_at_98"), // Matches existing block_98
    };

    let result = cp.insert(99, conflicting_block_99);

    // Expected: Heights 100, 101, 102 should be purged because block_100's
    // prev_blockhash conflicts with the new block_99's hash
    assert_eq!(
        result.height(),
        99,
        "tip should be at height 99 after purging higher checkpoints"
    );

    // Check that only 98 and 99 remain (plus placeholder at 97)
    assert_eq!(
        result.iter().count(),
        3,
        "should have 3 checkpoints (97 placeholder, 98, 99)"
    );

    // Verify height 99 has the new conflicting block
    let height_99 = result.get(99).expect("checkpoint at 99 should exist");
    assert_eq!(height_99.hash(), conflicting_block_99.blockhash);
    assert!(
        height_99.data_ref().is_some(),
        "checkpoint at 99 should have data"
    );

    // Verify height 98 remains unchanged
    let height_98 = result.get(98).expect("checkpoint at 98 should exist");
    assert_eq!(height_98.hash(), block_98.blockhash);
    assert!(
        height_98.data_ref().is_some(),
        "checkpoint at 98 should have data"
    );

    // Verify heights 100, 101, 102 are purged
    assert!(
        result.get(100).is_none(),
        "checkpoint at 100 should be purged"
    );
    assert!(
        result.get(101).is_none(),
        "checkpoint at 101 should be purged"
    );
    assert!(
        result.get(102).is_none(),
        "checkpoint at 102 should be purged"
    );
}

/// Test inserting between checkpoints with conflicts on both sides.
///
/// When inserting at height between two checkpoints where the inserted data's `prev_blockhash`
/// conflicts with the lower checkpoint and its `blockhash` conflicts with the upper checkpoint's
/// `prev_blockhash`, both checkpoints should be handled: lower displaced, upper purged.
///
/// Expected: Checkpoint at 4 displaced with placeholder, checkpoint at 6 purged.
#[test]
fn checkpoint_insert_between_conflicting_both_sides() {
    // Create checkpoints at heights 4 and 6
    let block_4 = TestBlock {
        blockhash: hash!("block_at_4"),
        prev_blockhash: hash!("block_at_3"),
    };
    let block_6 = TestBlock {
        blockhash: hash!("block_at_6"),
        prev_blockhash: hash!("block_at_5_original"), // This will conflict with inserted block 5
    };

    let cp = CheckPoint::from_blocks(vec![(4, block_4), (6, block_6)])
        .expect("should create valid checkpoint chain");

    // Verify initial state
    assert_eq!(cp.iter().count(), 4); // 3 (placeholder), 4, 5 (placeholder from 6's prev), 6

    // Insert at height 5 with conflicts on both sides
    let block_5_conflicting = TestBlock {
        blockhash: hash!("block_at_5_new"), // Conflicts with block_6.prev_blockhash
        prev_blockhash: hash!("different_block_at_4"), // Conflicts with block_4.blockhash
    };

    let result = cp.insert(5, block_5_conflicting);

    // Expected behavior:
    // - Checkpoint at 4 should be displaced with a placeholder containing block_5's prev_blockhash
    // - Checkpoint at 5 should have the inserted data
    // - Checkpoint at 6 should be purged due to prev_blockhash conflict

    // Verify height 4 is displaced with placeholder
    let height_4 = result.get(4).expect("checkpoint at 4 should exist");
    assert_eq!(height_4.height(), 4);
    assert_eq!(
        height_4.hash(),
        block_5_conflicting.prev_blockhash,
        "checkpoint at 4 should be displaced with block 5's prev_blockhash"
    );
    assert!(
        height_4.data_ref().is_none(),
        "checkpoint at 4 should be a placeholder"
    );

    // Verify height 5 has the inserted data
    let height_5 = result.get(5).expect("checkpoint at 5 should exist");
    assert_eq!(height_5.height(), 5);
    assert_eq!(height_5.hash(), block_5_conflicting.blockhash);
    assert!(
        height_5.data_ref().is_some(),
        "checkpoint at 5 should have data"
    );

    // Verify height 6 is purged
    assert!(
        result.get(6).is_none(),
        "checkpoint at 6 should be purged due to prev_blockhash conflict"
    );

    // Verify chain structure
    assert_eq!(result.height(), 5, "tip should be at height 5");
    // Should have: 3 (placeholder), 4 (placeholder), 5
    assert_eq!(result.iter().count(), 3, "should have 3 checkpoints");
}

/// Test that push returns Err(self) when trying to push at the same height.
#[test]
fn checkpoint_push_fails_same_height() {
    let cp: CheckPoint<BlockHash> = CheckPoint::new(100, hash!("block_100"));

    // Try to push at the same height (100)
    let result = cp.clone().push(100, hash!("another_block_100"));

    assert!(
        result.is_err(),
        "push should fail when height is same as current"
    );
    assert!(
        result.unwrap_err().eq_ptr(&cp),
        "should return self on error"
    );
}

/// Test that push returns Err(self) when trying to push at a lower height.
#[test]
fn checkpoint_push_fails_lower_height() {
    let cp: CheckPoint<BlockHash> = CheckPoint::new(100, hash!("block_100"));

    // Try to push at a lower height (99)
    let result = cp.clone().push(99, hash!("block_99"));

    assert!(
        result.is_err(),
        "push should fail when height is lower than current"
    );
    assert!(
        result.unwrap_err().eq_ptr(&cp),
        "should return self on error"
    );
}

/// Test that push returns Err(self) when prev_blockhash conflicts with self's hash.
#[test]
fn checkpoint_push_fails_conflicting_prev_blockhash() {
    let cp: CheckPoint<TestBlock> = CheckPoint::new(
        100,
        TestBlock {
            blockhash: hash!("block_100"),
            prev_blockhash: hash!("block_99"),
        },
    );

    // Create a block with a prev_blockhash that doesn't match cp's hash
    let conflicting_block = TestBlock {
        blockhash: hash!("block_101"),
        prev_blockhash: hash!("wrong_block_100"), // This conflicts with cp's hash
    };

    // Try to push at height 101 (contiguous) with conflicting prev_blockhash
    let result = cp.clone().push(101, conflicting_block);

    assert!(
        result.is_err(),
        "push should fail when prev_blockhash conflicts"
    );
    assert!(
        result.unwrap_err().eq_ptr(&cp),
        "should return self on error"
    );
}

/// Test that push succeeds when prev_blockhash matches self's hash for contiguous height.
#[test]
fn checkpoint_push_succeeds_matching_prev_blockhash() {
    let cp: CheckPoint<TestBlock> = CheckPoint::new(
        100,
        TestBlock {
            blockhash: hash!("block_100"),
            prev_blockhash: hash!("block_99"),
        },
    );

    // Create a block with matching prev_blockhash
    let matching_block = TestBlock {
        blockhash: hash!("block_101"),
        prev_blockhash: hash!("block_100"), // Matches cp's hash
    };

    // Push at height 101 with matching prev_blockhash
    let result = cp.push(101, matching_block);

    assert!(
        result.is_ok(),
        "push should succeed when prev_blockhash matches"
    );
    let new_cp = result.unwrap();
    assert_eq!(new_cp.height(), 101);
    assert_eq!(new_cp.hash(), hash!("block_101"));
}

/// Test that push creates a placeholder for non-contiguous heights with prev_blockhash.
#[test]
fn checkpoint_push_creates_placeholder_non_contiguous() {
    let cp: CheckPoint<TestBlock> = CheckPoint::new(
        100,
        TestBlock {
            blockhash: hash!("block_100"),
            prev_blockhash: hash!("block_99"),
        },
    );

    // Create a block at non-contiguous height with prev_blockhash
    let block_105 = TestBlock {
        blockhash: hash!("block_105"),
        prev_blockhash: hash!("block_104"),
    };

    // Push at height 105 (non-contiguous)
    let result = cp.push(105, block_105);

    assert!(
        result.is_ok(),
        "push should succeed for non-contiguous height"
    );
    let new_cp = result.unwrap();

    // Verify the tip is at 105
    assert_eq!(new_cp.height(), 105);
    assert_eq!(new_cp.hash(), hash!("block_105"));

    // Verify placeholder was created at 104
    let placeholder = new_cp.get(104).expect("should have placeholder at 104");
    assert_eq!(placeholder.hash(), hash!("block_104"));
    assert!(
        placeholder.data_ref().is_none(),
        "placeholder should have no data"
    );

    // Verify chain structure: 100 -> 99 (placeholder) -> 104 (placeholder) -> 105
    assert_eq!(new_cp.iter().count(), 4);
}
