#[macro_use]
mod common;

use bdk_core::CheckPoint;

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
        let cp_chain = CheckPoint::from_block_ids(blocks[..=i].iter().copied())
            .expect("must construct valid chain");

        for j in 0..=i {
            let block_to_insert = cp_chain
                .get(j as u32)
                .expect("cp of height must exist")
                .block_id();
            let new_cp_chain = cp_chain.clone().insert(block_to_insert);

            assert_eq!(
                new_cp_chain, cp_chain,
                "must not divert from original chain"
            );
            assert!(new_cp_chain.eq_ptr(&cp_chain), "pointers must still match");
        }
    }
}
