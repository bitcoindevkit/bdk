use bdk_core::CheckPoint;
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
