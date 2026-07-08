//! Arbitrary-driven generators for headers, chains, and block ids.

pub use arbitrary::*;

use std::collections::BTreeMap;

use bdk_chain::bitcoin::block::{Header, Version};
use bdk_chain::bitcoin::hashes::Hash;
use bdk_chain::bitcoin::{BlockHash, CompactTarget, TxMerkleNode};
use bdk_chain::local_chain::{ChangeSet, LocalChain};
use bdk_chain::{BlockId, CheckPoint, ToBlockHash};

/// Builds a [`BlockHash`] with arbitrary data.
pub fn block_hash(u: &mut Unstructured) -> arbitrary::Result<BlockHash> {
    Ok(BlockHash::from_byte_array(<[u8; 32]>::arbitrary(u)?))
}

/// Builds a [`Header`] with arbitrary fields on top of `prev_blockhash`.
pub fn header(u: &mut Unstructured, prev_blockhash: BlockHash) -> arbitrary::Result<Header> {
    Ok(Header {
        version: Version::from_consensus(i32::arbitrary(u)?),
        prev_blockhash,
        merkle_root: TxMerkleNode::from_byte_array(<[u8; 32]>::arbitrary(u)?),
        time: u32::arbitrary(u)?,
        bits: CompactTarget::from_consensus(u32::arbitrary(u)?),
        nonce: u32::arbitrary(u)?,
    })
}

/// Builds a [`Header`] from arbitrary data, that connects to one of the existing [`BlockId`]'s in
/// the [`LocalChain`].
pub fn connectable_header<D>(
    u: &mut Unstructured,
    chain: &LocalChain<D>,
) -> arbitrary::Result<(Header, u32)>
where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    let (prev_blockhash, height) = if u.ratio(3, 4)? {
        let block_ids: Vec<BlockId> = chain.iter_checkpoints().map(|cp| cp.block_id()).collect();
        let connect_at = u.choose(&block_ids)?;
        (connect_at.hash, connect_at.height.saturating_add(1))
    } else {
        (block_hash(u)?, u32::arbitrary(u)?)
    };
    let header = header(u, prev_blockhash)?;
    Ok((header, height))
}

/// Regenerates `headers` from `fork_height` upward with fresh arbitrary fields, keeping
/// `prev_blockhash` links intact so contiguous checkpoints remain valid.
pub fn reorg(
    u: &mut Unstructured,
    headers: &mut [Header],
    fork_height: usize,
) -> arbitrary::Result<()> {
    for height in fork_height..headers.len() {
        let prev_blockhash = match height.checked_sub(1) {
            Some(prev) => headers[prev].block_hash(),
            None => BlockHash::all_zeros(),
        };
        headers[height] = header(u, prev_blockhash)?;
    }
    Ok(())
}

/// Picks a header for an operation at `height`: usually the virtual chain's (shares hashes
/// with the chain under test), sometimes a foreign one (conflicts).
pub fn header_at(
    u: &mut Unstructured,
    headers: &[Header],
    height: usize,
) -> arbitrary::Result<Header> {
    if u.ratio(7, 8)? {
        Ok(headers[height])
    } else {
        let prev_blockhash = block_hash(u)?;
        header(u, prev_blockhash)
    }
}

/// Builds a [`LocalChain<D>`] from `blocks` via an arbitrarily chosen constructor.
///
/// Returns `None` when `blocks` does not form a valid chain (empty, missing genesis, or
/// inconsistent `prev_blockhash` links). On success, asserts that the constructed chain
/// agrees with `blocks` on tip and genesis.
pub fn chain_from_blocks<D>(
    u: &mut Unstructured,
    blocks: BTreeMap<u32, D>,
) -> arbitrary::Result<Option<LocalChain<D>>>
where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    let constructed = match u.int_in_range(0..=2)? {
        0 => LocalChain::from_blocks(blocks.clone()).ok(),
        1 => {
            let changeset = ChangeSet {
                blocks: blocks
                    .iter()
                    .map(|(&height, data)| (height, Some(data.clone())))
                    .collect(),
            };
            LocalChain::from_changeset(changeset).ok()
        }
        _ => CheckPoint::from_blocks(blocks.clone())
            .ok()
            .and_then(|tip| LocalChain::from_tip(tip).ok()),
    };
    let chain = match constructed {
        Some(chain) => chain,
        None => return Ok(None),
    };

    let (&tip_height, tip_data) = blocks.last_key_value().expect("chain is non-empty");
    assert_eq!(chain.tip().block_id().height, tip_height);
    assert_eq!(chain.tip().block_id().hash, tip_data.to_blockhash());
    assert_eq!(chain.genesis_hash(), blocks[&0].to_blockhash());

    Ok(Some(chain))
}

/// Builds a [`LocalChain<BlockHash>`] from an arbitrary data.
pub fn blockhash_chain(u: &mut Unstructured) -> arbitrary::Result<Option<LocalChain>> {
    let blocks: BTreeMap<u32, BlockHash> = BTreeMap::<u32, [u8; 32]>::arbitrary(u)?
        .into_iter()
        .map(|(height, hash)| (height, BlockHash::from_byte_array(hash)))
        .collect();
    chain_from_blocks(u, blocks)
}

/// Builds a `LocalChain<Header>` occupying an arbitrary subset of the virtual chain's
/// heights (genesis always included), via an arbitrarily chosen constructor.
pub fn header_chain(
    u: &mut Unstructured,
    headers: &[Header],
) -> arbitrary::Result<Option<LocalChain<Header>>> {
    // The mask is 32 bits wide, so occupancy repeats for heights >= 32.
    let occupied_mask = u32::arbitrary(u)? | 1;
    let blocks: BTreeMap<u32, Header> = headers
        .iter()
        .enumerate()
        .map(|(height, header)| (height as u32, *header))
        .filter(|(height, _)| occupied_mask & (1u32 << (height % 32)) != 0)
        .collect();
    chain_from_blocks(u, blocks)
}

/// Builds a `ChangeSet<D>` for `apply_changeset`.
///
/// Heights are usually drawn from the chain's checkpoints (or just above one), so entries land
/// on the boundaries that matter: replacing a checkpoint, removing one, extending past the tip.
/// Each entry is either an insertion, with data from `data_at`, or a removal (`None`).
pub fn changeset<'a, D, F>(
    u: &mut Unstructured<'a>,
    chain: &LocalChain<D>,
    mut data_at: F,
) -> arbitrary::Result<ChangeSet<D>>
where
    D: ToBlockHash + std::fmt::Debug + Clone,
    F: FnMut(&mut Unstructured<'a>, u32) -> arbitrary::Result<D>,
{
    let heights: Vec<u32> = chain.iter_checkpoints().map(|cp| cp.height()).collect();
    let entry_count = u.int_in_range(1..=4)?;
    let mut blocks = BTreeMap::new();
    for _ in 0..entry_count {
        let height = match u.int_in_range(0..=2)? {
            0 => *u.choose(&heights)?,
            1 => u.choose(&heights)?.saturating_add(1),
            _ => u32::arbitrary(u)?,
        };
        let data = if u.ratio(3, 4)? {
            Some(data_at(u, height)?)
        } else {
            None
        };
        blocks.insert(height, data);
    }
    Ok(ChangeSet { blocks })
}

/// Picks a block id for an operation: a checkpoint of `chain`, one of `headers` (when
/// non-empty), or an arbitrary one.
pub fn block_id<D>(
    u: &mut Unstructured,
    chain: &LocalChain<D>,
    headers: &[Header],
) -> arbitrary::Result<BlockId>
where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    let variant_count = if headers.is_empty() { 1 } else { 2 };
    match u.int_in_range(0..=variant_count)? {
        0 => {
            let block_ids: Vec<BlockId> =
                chain.iter_checkpoints().map(|cp| cp.block_id()).collect();
            Ok(*u.choose(&block_ids)?)
        }
        1 if !headers.is_empty() => {
            let height = u.int_in_range(0..=headers.len() - 1)?;
            Ok(BlockId {
                height: height as u32,
                hash: headers[height].block_hash(),
            })
        }
        _ => Ok(BlockId {
            height: u32::arbitrary(u)?,
            hash: block_hash(u)?,
        }),
    }
}
