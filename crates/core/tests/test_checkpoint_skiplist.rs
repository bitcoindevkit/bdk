use bdk_core::CheckPoint;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;

#[test]
fn test_skiplist_indices() {
    // Build a chain long enough to hold multiple skip pointers (indices 1000..=5000).
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());
    assert_eq!(cp.index(), 0);

    for height in 1..=5000u32 {
        let hash = BlockHash::from_byte_array([(height % 256) as u8; 32]);
        cp = cp.push(height, hash).unwrap();
        assert_eq!(cp.index(), height);
    }
    assert_eq!(cp.index(), 5000);

    // Skip pointers are expected at indices 1000, 2000, 3000, 4000, 5000, each pointing 1000 back.
    // Intermediate indices should not have skip pointers.
    for target_index in [5000u32, 4000, 3000, 2000, 1000] {
        let node = cp.get(target_index).expect("checkpoint exists");
        let skip = node
            .skip()
            .unwrap_or_else(|| panic!("expected skip pointer at index {target_index}"));
        assert_eq!(skip.index(), target_index - 1000);
    }

    for non_skip_index in [500u32, 1500, 2500, 4999] {
        let node = cp.get(non_skip_index).expect("checkpoint exists");
        assert!(
            node.skip().is_none(),
            "unexpected skip pointer at index {non_skip_index}"
        );
    }
}

#[test]
fn test_skiplist_floor_at() {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    // Create sparse chain with gaps
    for height in [10, 50, 100, 150, 200, 300, 400, 500] {
        let hash = BlockHash::from_byte_array([height as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Test floor_at with skip pointers
    let floor = cp.floor_at(250).unwrap();
    assert_eq!(floor.height(), 200);

    let floor = cp.floor_at(99).unwrap();
    assert_eq!(floor.height(), 50);

    let floor = cp.floor_at(500).unwrap();
    assert_eq!(floor.height(), 500);

    let floor = cp.floor_at(600).unwrap();
    assert_eq!(floor.height(), 500);
}

#[test]
fn test_skiplist_insert_maintains_indices() {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    // Build initial chain
    for height in [10, 20, 30, 40, 50] {
        let hash = BlockHash::from_byte_array([height as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Insert a block in the middle
    let hash = BlockHash::from_byte_array([25; 32]);
    cp = cp.insert(25, hash);

    // Check that indices are maintained correctly
    let check = cp.get(50).unwrap();
    assert_eq!(check.index(), 6); // 0, 10, 20, 25, 30, 40, 50

    let check = cp.get(25).unwrap();
    assert_eq!(check.index(), 3);

    // Check the full chain has correct indices
    let mut current = cp.clone();
    let expected_heights = [50, 40, 30, 25, 20, 10, 0];
    let expected_indices = [6, 5, 4, 3, 2, 1, 0];

    for (expected_height, expected_index) in expected_heights.iter().zip(expected_indices.iter()) {
        assert_eq!(current.height(), *expected_height);
        assert_eq!(current.index(), *expected_index);
        if *expected_height > 0 {
            current = current.prev().unwrap();
        }
    }
}

#[test]
fn test_skiplist_range_uses_skip_pointers() {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    // Create a chain with 500 checkpoints
    for height in 1..=500 {
        let hash = BlockHash::from_byte_array([(height % 256) as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Test range iteration
    let range_items: Vec<_> = cp.range(100..=200).collect();
    assert_eq!(range_items.len(), 101);
    assert_eq!(range_items.first().unwrap().height(), 200);
    assert_eq!(range_items.last().unwrap().height(), 100);

    // Test open range
    let range_items: Vec<_> = cp.range(450..).collect();
    assert_eq!(range_items.len(), 51);
    assert_eq!(range_items.first().unwrap().height(), 500);
    assert_eq!(range_items.last().unwrap().height(), 450);
}

#[test]
fn test_range_edge_cases() {
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    // Create sparse chain: 0, 100, 200, 300, 400, 500
    for i in 1..=5 {
        let height = i * 100;
        let hash = BlockHash::from_byte_array([i as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Empty range (start > end)
    #[allow(clippy::reversed_empty_ranges)]
    let empty: Vec<_> = cp.range(300..200).collect();
    assert!(empty.is_empty());

    // Single element range
    let single: Vec<_> = cp.range(300..=300).collect();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].height(), 300);

    // Range with non-existent bounds (150..250)
    let partial: Vec<_> = cp.range(150..250).collect();
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0].height(), 200);

    // Exclusive end bound (100..300 includes 100 and 200, but not 300)
    let exclusive: Vec<_> = cp.range(100..300).collect();
    assert_eq!(exclusive.len(), 2);
    assert_eq!(exclusive[0].height(), 200);
    assert_eq!(exclusive[1].height(), 100);

    // Unbounded range (..)
    let all: Vec<_> = cp.range(..).collect();
    assert_eq!(all.len(), 6);
    assert_eq!(all.first().unwrap().height(), 500);
    assert_eq!(all.last().unwrap().height(), 0);

    // Range beyond chain bounds
    let beyond: Vec<_> = cp.range(600..700).collect();
    assert!(beyond.is_empty());

    // Range from genesis
    let from_genesis: Vec<_> = cp.range(0..=200).collect();
    assert_eq!(from_genesis.len(), 3);
    assert_eq!(from_genesis[0].height(), 200);
    assert_eq!(from_genesis[2].height(), 0);
}
