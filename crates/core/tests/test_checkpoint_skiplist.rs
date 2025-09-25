use bdk_core::CheckPoint;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;

#[test]
fn test_skiplist_indices() {
    // Create a long chain to test skiplist
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());
    assert_eq!(cp.index(), 0);

    for height in 1..=500 {
        let hash = BlockHash::from_byte_array([height as u8; 32]);
        cp = cp.push(height, hash).unwrap();
        assert_eq!(cp.index(), height);
    }

    // Test that skip pointers are set correctly
    // At index 100, 200, 300, 400, 500 we should have skip pointers
    assert_eq!(cp.index(), 500);

    // Navigate to index 400 and check skip pointer
    let mut current = cp.clone();
    for _ in 0..100 {
        current = current.prev().unwrap();
    }
    assert_eq!(current.index(), 400);

    // Check that skip pointer exists at index 400
    if let Some(skip) = current.skip() {
        assert_eq!(skip.index(), 300);
    } else {
        panic!("Expected skip pointer at index 400");
    }

    // Navigate to index 300 and check skip pointer
    for _ in 0..100 {
        current = current.prev().unwrap();
    }
    assert_eq!(current.index(), 300);

    if let Some(skip) = current.skip() {
        assert_eq!(skip.index(), 200);
    } else {
        panic!("Expected skip pointer at index 300");
    }

    // Navigate to index 100 and check skip pointer
    for _ in 0..200 {
        current = current.prev().unwrap();
    }
    assert_eq!(current.index(), 100);

    if let Some(skip) = current.skip() {
        assert_eq!(skip.index(), 0);
    } else {
        panic!("Expected skip pointer at index 100");
    }
}

#[test]
fn test_skiplist_get_performance() {
    // Create a very long chain
    let mut cp = CheckPoint::new(0, BlockHash::all_zeros());

    for height in 1..=1000 {
        let hash = BlockHash::from_byte_array([(height % 256) as u8; 32]);
        cp = cp.push(height, hash).unwrap();
    }

    // Test that get() can find checkpoints efficiently
    // This should use skip pointers to navigate quickly

    // Verify the chain was built correctly
    assert_eq!(cp.height(), 1000);
    assert_eq!(cp.index(), 1000);

    // Find checkpoint near the beginning
    if let Some(found) = cp.get(50) {
        assert_eq!(found.height(), 50);
        assert_eq!(found.index(), 50);
    } else {
        // Debug: print the first few checkpoints
        let mut current = cp.clone();
        println!("First 10 checkpoints:");
        for _ in 0..10 {
            println!("Height: {}, Index: {}", current.height(), current.index());
            if let Some(prev) = current.prev() {
                current = prev;
            } else {
                break;
            }
        }
        panic!("Could not find checkpoint at height 50");
    }

    // Find checkpoint in the middle
    if let Some(found) = cp.get(500) {
        assert_eq!(found.height(), 500);
        assert_eq!(found.index(), 500);
    } else {
        panic!("Could not find checkpoint at height 500");
    }

    // Find checkpoint near the end
    if let Some(found) = cp.get(950) {
        assert_eq!(found.height(), 950);
        assert_eq!(found.index(), 950);
    } else {
        panic!("Could not find checkpoint at height 950");
    }

    // Test non-existent checkpoint
    assert!(cp.get(1001).is_none());
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
