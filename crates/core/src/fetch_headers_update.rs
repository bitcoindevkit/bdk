//! Helpers for building sparse [`CheckPoint<Header>`] updates from on-demand height fetches.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use bitcoin::block::Header;

use crate::CheckPoint;

/// For each requested height, the nearest local checkpoint strictly above it (if any).
///
/// Bridge blocks connect sparse gaps so [`bdk_chain::local_chain::LocalChain::apply_update`]
/// can merge the update unambiguously.
pub fn bridge_heights(
    local_tip: &CheckPoint<Header>,
    agreement_height: u32,
    requested_heights: &[u32],
) -> BTreeSet<u32> {
    let mut bridges = BTreeSet::new();
    for &h in requested_heights {
        for local_cp in local_tip.iter() {
            let lh = local_cp.height();
            if lh > h && lh > agreement_height {
                bridges.insert(lh);
                break;
            }
        }
    }
    bridges
}

/// Assemble a sparse header checkpoint update after the backend has resolved agreement and fetches.
///
/// `conflicts` must be in descending height order (as collected from an agreement walk from tip).
pub fn build_fetch_headers_update(
    agreement: CheckPoint<Header>,
    conflicts: Vec<(u32, Header)>,
    fetched: BTreeMap<u32, Header>,
    local_tip: &CheckPoint<Header>,
    bridge_heights: &BTreeSet<u32>,
) -> CheckPoint<Header> {
    let mut tip = agreement;

    if !conflicts.is_empty() {
        tip = tip
            .extend(conflicts.into_iter().rev())
            .expect("conflict headers must be in ascending height order");
    }

    for (height, header) in fetched {
        if tip.get(height).is_none() {
            tip = tip.insert(height, header);
        }
    }

    for &height in bridge_heights {
        if tip.get(height).is_none() {
            if let Some(cp) = local_tip.get(height) {
                tip = tip.insert(height, cp.data());
            }
        }
    }

    tip
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::block::Header;
    use bitcoin::hashes::Hash;
    use bitcoin::BlockHash;
    use bitcoin::CompactTarget;

    fn dummy_header(prev: BlockHash, n: u32) -> Header {
        Header {
            version: bitcoin::block::Version::from_consensus(1),
            prev_blockhash: prev,
            merkle_root: bitcoin::hashes::Hash::hash(&n.to_le_bytes()),
            time: 1_000 + n,
            bits: CompactTarget::from_consensus(0),
            nonce: n,
        }
    }

    fn sparse_local_chain() -> CheckPoint<Header> {
        let h0 = dummy_header(BlockHash::all_zeros(), 0);
        let h1 = dummy_header(h0.block_hash(), 1);
        let h5 = dummy_header(h1.block_hash(), 5);
        CheckPoint::new(0, h0)
            .insert(1, h1)
            .insert(5, h5)
    }

    #[test]
    fn bridge_heights_picks_nearest_above() {
        let local = sparse_local_chain();
        let bridges = bridge_heights(&local, 1, &[4]);
        assert_eq!(bridges, BTreeSet::from([5]));
    }
}
