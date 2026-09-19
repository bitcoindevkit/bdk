//! Fuzzes `LocalChain<Header>`, where checkpoint data knows its `prev_blockhash`.
//!
//! Unlike the `BlockHash`-based target, gaps between checkpoints imply *placeholder*
//! entries (`CheckPointEntry::Placeholder`), exercising the placeholder resolution
//! paths in `apply_update` and `CheckPoint::entry_iter`.
//!
//! Headers are derived from a "virtual chain" of properly linked headers. Each operation
//! draws headers from it (or arbitrary foreign ones), so operations share block hashes
//! with the chain under test (connection points, placeholder fills) and reorgs regenerate
//! headers above an arbitrary fork height (conflicts, invalidation).
#![cfg_attr(feature = "libfuzzer_fuzz", no_main)]

use std::collections::BTreeMap;

use bdk_chain::bitcoin::block::{Header, Version};
use bdk_chain::bitcoin::hashes::Hash;
use bdk_chain::bitcoin::{BlockHash, CompactTarget, TxMerkleNode};
use bdk_chain::local_chain::LocalChain;
use bdk_chain::CheckPointEntry;
use bdk_fuzz::chain::arbitrary::{self, Arbitrary, Unstructured};
use bdk_fuzz::chain::checks::{
    assert_changeset_against_chains, assert_changeset_applied, assert_checkpoint_order,
    assert_initial_changeset_roundtrip,
};

const MAX_HEIGHT: u32 = 32;

/// An operation to perform against the chain under test.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum Op {
    /// `apply_update` with an independently constructed subset of the virtual chain.
    ApplyUpdate,
    /// `insert_block` with a virtual chain header (or sometimes a foreign one).
    InsertBlock,
    /// `disconnect_from` a checkpoint, a virtual chain block, or an arbitrary block id.
    DisconnectFrom,
    /// `apply_update` with an update derived by inserting into the chain's own tip, so
    /// the update shares `Arc` nodes with the original and exercises `merge_chains`'
    /// `eq_ptr` fast path.
    ApplyDerivedUpdate,
    /// `apply_changeset` with a mix of insertions (usually virtual chain headers, so
    /// `prev_blockhash` links can resolve) and removals.
    ApplyChangeSet,
}

/// Checks checkpoint and entry invariants, including placeholder consistency.
fn assert_chain(chain: &LocalChain<Header>) {
    assert_checkpoint_order(chain);
    assert_initial_changeset_roundtrip(chain);
    let occupied: BTreeMap<u32, BlockHash> = chain
        .iter_checkpoints()
        .map(|cp| (cp.height(), cp.hash()))
        .collect();

    let mut entry_heights = Vec::new();
    for entry in chain.tip().entry_iter() {
        entry_heights.push(entry.height());
        match &entry {
            CheckPointEntry::Placeholder {
                block_id,
                checkpoint_above,
            } => {
                assert!(
                    !occupied.contains_key(&block_id.height),
                    "placeholder height must not hold a real checkpoint"
                );
                assert_eq!(checkpoint_above.height(), block_id.height + 1);
                assert_eq!(checkpoint_above.data_ref().prev_blockhash, block_id.hash);
            }
            CheckPointEntry::Occupied(cp) => {
                assert_eq!(occupied.get(&cp.height()), Some(&cp.hash()));
            }
        }
    }
    assert!(
        entry_heights.windows(2).all(|w| w[0] > w[1]),
        "entry heights must be strictly decreasing from tip"
    );
    let occupied_entries = entry_heights
        .iter()
        .filter(|h| occupied.contains_key(h))
        .count();
    assert_eq!(
        occupied_entries,
        occupied.len(),
        "entry_iter must yield every real checkpoint"
    );
}

fn do_test(data: &[u8]) {
    let mut u = Unstructured::new(data);

    let height_count = match u.int_in_range(1..=MAX_HEIGHT) {
        Ok(count) => count,
        Err(_) => return,
    };
    let mut headers = vec![
        Header {
            version: Version::NO_SOFT_FORK_SIGNALLING,
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: 0,
            bits: CompactTarget::from_consensus(0),
            nonce: 0,
        };
        height_count as usize
    ];
    if arbitrary::reorg(&mut u, &mut headers, 0).is_err() {
        return;
    }

    let op_count = match u.int_in_range(1..=16) {
        Ok(count) => count,
        Err(_) => return,
    };

    let mut chain: Option<LocalChain<Header>> = None;
    for _ in 0..op_count {
        if chain.is_none() {
            match arbitrary::header_chain(&mut u, &headers) {
                Ok(Some(initial)) => chain = Some(initial),
                Ok(None) => continue,
                Err(_) => break,
            }
            continue;
        }
        let chain = chain.as_mut().expect("initialized above");

        let op = match Op::arbitrary(&mut u) {
            Ok(op) => op,
            Err(_) => break,
        };
        let pre = chain.clone();
        let genesis = chain.genesis_hash();
        match op {
            Op::ApplyUpdate => {
                let update = match arbitrary::header_chain(&mut u, &headers) {
                    Ok(Some(update)) => update,
                    Ok(None) => continue,
                    Err(_) => break,
                };
                let result = chain.apply_update(update.tip());
                assert_changeset_against_chains(pre, chain, &result);
            }
            Op::InsertBlock => {
                let (height, header) =
                    match u.int_in_range(0..=headers.len() - 1).and_then(|height| {
                        arbitrary::header_at(&mut u, &headers, height).map(|h| (height, h))
                    }) {
                        Ok(block) => block,
                        Err(_) => break,
                    };
                let result = chain.insert_block(height as u32, header);
                assert_changeset_against_chains(pre, chain, &result);
                match &result {
                    Ok(_) => {
                        assert_eq!(
                            chain.get(height as u32).map(|cp| cp.hash()),
                            Some(header.block_hash())
                        );
                    }
                    Err(err) => {
                        assert_eq!(
                            chain.get(err.height).map(|cp| cp.hash()),
                            Some(err.original_hash),
                            "insert conflict must report the existing checkpoint"
                        );
                    }
                }
            }
            Op::DisconnectFrom => {
                let block_id = match arbitrary::block_id(&mut u, chain, &headers) {
                    Ok(block_id) => block_id,
                    Err(_) => break,
                };
                let result = chain.disconnect_from(block_id);
                assert_changeset_against_chains(pre, chain, &result);
                match &result {
                    Ok(changeset) if !changeset.blocks.is_empty() => {
                        assert!(chain.tip().height() < block_id.height);
                    }
                    Ok(_) => {}
                    Err(_missing_genesis) => {
                        assert_eq!(block_id.height, 0);
                        assert_eq!(block_id.hash, chain.genesis_hash());
                    }
                }
            }
            Op::ApplyDerivedUpdate => {
                // Heights 0 and 1 are excluded: after a reorg the virtual headers may
                // imply a different genesis, which `CheckPoint::insert` rejects with a
                // panic.
                if headers.len() < 3 {
                    continue;
                }
                let params: arbitrary::Result<_> = (|| {
                    let height = u.int_in_range(2..=headers.len() - 1)?;
                    let header = arbitrary::header_at(&mut u, &headers, height)?;
                    Ok((height as u32, header))
                })();
                let (height, header) = match params {
                    Ok(params) => params,
                    Err(_) => break,
                };
                let update_tip = chain.tip().insert(height, header);
                let result = chain.apply_update(update_tip);
                assert_changeset_against_chains(pre, chain, &result);
                if result.is_ok() {
                    assert_eq!(
                        chain.get(height).map(|cp| cp.hash()),
                        Some(header.block_hash())
                    );
                }
            }
            Op::ApplyChangeSet => {
                let changeset = match arbitrary::changeset(&mut u, chain, |u, height| {
                    if (height as usize) < headers.len() {
                        return arbitrary::header_at(u, &headers, height as usize);
                    }
                    let prev_blockhash = arbitrary::block_hash(u)?;
                    arbitrary::header(u, prev_blockhash)
                }) {
                    Ok(changeset) => changeset,
                    Err(_) => break,
                };
                let result = chain.apply_changeset(&changeset);
                assert_changeset_applied(&pre, chain, &changeset, &result);
            }
        }
        assert_eq!(
            genesis,
            chain.genesis_hash(),
            "no operation may replace the genesis block"
        );
        assert_chain(chain);

        let fork_height = match u.int_in_range(0..=headers.len()) {
            Ok(fork_height) => fork_height,
            Err(_) => break,
        };
        if fork_height < headers.len()
            && arbitrary::reorg(&mut u, &mut headers, fork_height).is_err()
        {
            break;
        }
    }

    if let Some(chain) = chain {
        assert_chain(&chain);
    }
}

bdk_fuzz::fuzz_main!(do_test);
