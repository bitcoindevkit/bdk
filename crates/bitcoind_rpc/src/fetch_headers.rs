//! On-demand header fetch at caller-requested heights.

use bdk_core::collections::{BTreeMap, BTreeSet};
use bdk_core::{bridge_heights, build_fetch_headers_update, CheckPoint};
use bitcoin::block::Header;
use bitcoincore_rpc::RpcApi;
use core::ops::Deref;

use alloc::vec::Vec;

/// Error returned by [`fetch_headers_at_heights`].
#[derive(Debug)]
pub enum FetchHeadersError {
    /// An RPC error occurred.
    Rpc(bitcoincore_rpc::Error),
    /// No common ancestor was found between `local_tip` and the node's best chain.
    NoAgreement,
}

impl core::fmt::Display for FetchHeadersError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rpc(err) => write!(f, "bitcoind rpc error: {err}"),
            Self::NoAgreement => write!(f, "no agreement point with bitcoind best chain"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for FetchHeadersError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rpc(err) => Some(err),
            Self::NoAgreement => None,
        }
    }
}

impl From<bitcoincore_rpc::Error> for FetchHeadersError {
    fn from(err: bitcoincore_rpc::Error) -> Self {
        Self::Rpc(err)
    }
}

/// Fetch a reorg-aware [`CheckPoint<Header>`] update covering `heights`.
///
/// Heights already present in `local_tip` are not fetched from the network. When every requested
/// height is already known and no reorg is detected, returns `local_tip` unchanged (a no-op update
/// that applies with an empty [`bdk_chain::local_chain::ChangeSet`]).
///
/// The returned checkpoint may include bridge blocks and reorg replacement headers beyond the
/// requested set — only what is needed for [`bdk_chain::local_chain::LocalChain::apply_update`]
/// to connect unambiguously.
pub fn fetch_headers_at_heights<C>(
    client: &C,
    local_tip: CheckPoint<Header>,
    heights: impl IntoIterator<Item = u32>,
) -> Result<CheckPoint<Header>, FetchHeadersError>
where
    C: RpcApi,
{
    let requested: BTreeSet<u32> = heights.into_iter().collect();
    let requested_vec: Vec<u32> = requested.iter().copied().collect();
    let missing: Vec<u32> = requested
        .iter()
        .copied()
        .filter(|h| local_tip.get(*h).is_none())
        .collect();

    let tip_height = client.get_block_count()? as u32;

    let mut point_of_agreement = None;
    let mut conflicts = Vec::new();

    for local_cp in local_tip.iter() {
        let height = local_cp.height();
        if height > tip_height {
            continue;
        }
        let remote_hash = client.get_block_hash(height as u64)?;
        if remote_hash == local_cp.hash() {
            point_of_agreement = Some(local_cp);
            break;
        }
        let header = client.get_block_header(&remote_hash)?;
        conflicts.push((height, header));
    }

    let agreement = match point_of_agreement {
        Some(cp) => cp,
        None => return Err(FetchHeadersError::NoAgreement),
    };

    let reorg_detected = !conflicts.is_empty();
    if missing.is_empty() && !reorg_detected {
        return Ok(local_tip);
    }

    let mut fetched = BTreeMap::new();
    for &height in &missing {
        if height > tip_height {
            return Err(FetchHeadersError::Rpc(
                client.get_block_hash(height as u64).unwrap_err(),
            ));
        }
        let hash = client.get_block_hash(height as u64)?;
        let header = client.get_block_header(&hash)?;
        fetched.insert(height, header);
    }

    let bridges = bridge_heights(&local_tip, agreement.height(), &requested_vec);
    Ok(build_fetch_headers_update(
        agreement, conflicts, fetched, &local_tip, &bridges,
    ))
}

/// Fetch headers at caller-requested heights using a dereferencing client wrapper.
pub fn fetch_headers_at_heights_with<C>(
    client: C,
    local_tip: CheckPoint<Header>,
    heights: impl IntoIterator<Item = u32>,
) -> Result<CheckPoint<Header>, FetchHeadersError>
where
    C: Deref,
    C::Target: RpcApi,
{
    fetch_headers_at_heights(&*client, local_tip, heights)
}
