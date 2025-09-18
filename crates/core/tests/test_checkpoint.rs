use bdk_core::{CheckPoint, ToBlockHash, ToBlockTime};
use bdk_testenv::{block_id, hash};
use bitcoin::hashes::Hash;
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

/// Test helper: A block data type that includes timestamp
/// Fields are (height, time)
#[derive(Debug, Clone, Copy)]
struct BlockWithTime(u32, u32);

impl ToBlockHash for BlockWithTime {
    fn to_blockhash(&self) -> BlockHash {
        // Generate a deterministic hash from the height
        let hash_bytes = bitcoin::hashes::sha256d::Hash::hash(&self.0.to_le_bytes());
        BlockHash::from_raw_hash(hash_bytes)
    }
}

impl ToBlockTime for BlockWithTime {
    fn to_blocktime(&self) -> u32 {
        self.1
    }
}

#[test]
fn test_median_time_past_with_timestamps() {
    // Create a chain with 12 blocks (heights 0-11) with incrementing timestamps
    let blocks: Vec<_> = (0..=11)
        .map(|i| (i, BlockWithTime(i, 1000 + i * 10)))
        .collect();

    let cp = CheckPoint::from_blocks(blocks).expect("must construct valid chain");

    // Height 11: 11 previous blocks (11..=1), pseudo-median at index 6 = 1060
    assert_eq!(cp.median_time_past(), Some(1060));

    // Height 10: 11 previous blocks (10..=0), pseudo-median at index 5 = 1050
    assert_eq!(cp.get(10).unwrap().median_time_past(), Some(1050));

    // Height 5: 6 previous blocks (5..=0), pseudo-median at index 3 = 1030
    assert_eq!(cp.get(5).unwrap().median_time_past(), Some(1030));

    // Height 3: 4 previous blocks (3..=0), pseudo-median at index 2 = 1020
    assert_eq!(cp.get(3).unwrap().median_time_past(), Some(1020));

    // Height 0: 1 block at index 0 = 1000
    assert_eq!(cp.get(0).unwrap().median_time_past(), Some(1000));
}

#[test]
fn test_previous_median_time_past_edge_cases() {
    // Test with minimum required blocks (11)
    let blocks: Vec<_> = (0..=10)
        .map(|i| (i, BlockWithTime(i, 1000 + i * 100)))
        .collect();

    let cp = CheckPoint::from_blocks(blocks).expect("must construct valid chain");

    // At height 10: next_mtp uses all 11 blocks (0-10)
    // Times: [1000, 1100, 1200, 1300, 1400, 1500, 1600, 1700, 1800, 1900, 2000]
    // Median at index 5 = 1500
    assert_eq!(cp.median_time_past(), Some(1500));

    // At height 9: mtp uses blocks 0-9 (10 blocks)
    // Times: [1000, 1100, 1200, 1300, 1400, 1500, 1600, 1700, 1800, 1900]
    // Median at index 5 = 1400
    assert_eq!(cp.get(9).unwrap().median_time_past(), Some(1500));

    // Test sparse chain where next_mtp returns None due to missing blocks
    let sparse = vec![
        (0, BlockWithTime(0, 1000)),
        (5, BlockWithTime(5, 1050)),
        (10, BlockWithTime(10, 1100)),
    ];
    let sparse_cp = CheckPoint::from_blocks(sparse).expect("must construct valid chain");

    // At height 10: next_mtp needs blocks 0-10 but many are missing
    assert_eq!(sparse_cp.median_time_past(), None);
}

#[test]
fn test_mtp_with_non_monotonic_times() {
    // Test both methods with shuffled timestamps
    let blocks = vec![
        (0, BlockWithTime(0, 1500)),
        (1, BlockWithTime(1, 1200)),
        (2, BlockWithTime(2, 1800)),
        (3, BlockWithTime(3, 1100)),
        (4, BlockWithTime(4, 1900)),
        (5, BlockWithTime(5, 1300)),
        (6, BlockWithTime(6, 1700)),
        (7, BlockWithTime(7, 1400)),
        (8, BlockWithTime(8, 1600)),
        (9, BlockWithTime(9, 1000)),
        (10, BlockWithTime(10, 2000)),
        (11, BlockWithTime(11, 1650)),
    ];

    let cp = CheckPoint::from_blocks(blocks).expect("must construct valid chain");

    // Height 10:
    // mtp uses blocks 0-10: sorted
    // [1000,1100,1200,1300,1400,1500,1600,1700,1800,1900,2000] Median at index 5 = 1500
    assert_eq!(cp.get(10).unwrap().median_time_past(), Some(1500));

    // Height 11:
    // mtp uses blocks 1-11: sorted
    // [1000,1100,1200,1300,1400,1600,1650,1700,1800,1900,2000] Median at index 5 = 1600
    assert_eq!(cp.median_time_past(), Some(1600));

    // Test with smaller chain to verify sorting at different heights
    let cp3 = cp.get(3).unwrap();
    // Height 3: timestamps [1100, 1800, 1200, 1500] -> sorted [1100, 1200, 1500, 1800]
    // Pseudo-median at index 2 = 1500
    assert_eq!(cp3.median_time_past(), Some(1500));
}

#[test]
fn test_mtp_sparse_chain() {
    // Sparse chain missing required sequential blocks
    let blocks = vec![
        (0, BlockWithTime(0, 1000)),
        (3, BlockWithTime(3, 1030)),
        (7, BlockWithTime(7, 1070)),
        (11, BlockWithTime(11, 1110)),
        (15, BlockWithTime(15, 1150)),
    ];

    let cp = CheckPoint::from_blocks(blocks).expect("must construct valid chain");

    // All heights should return None due to missing sequential blocks
    assert_eq!(cp.median_time_past(), None);
    assert_eq!(cp.get(11).unwrap().median_time_past(), None);
}
