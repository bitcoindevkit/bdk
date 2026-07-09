//! On-demand header fetch at caller-requested heights.

use bdk_core::bitcoin::block::Header;
use bdk_core::collections::{BTreeMap, BTreeSet};
use bdk_core::{bridge_heights, build_fetch_headers_update, CheckPoint};

type Error = Box<esplora_client::Error>;

/// Fetch a reorg-aware [`CheckPoint<Header>`] update covering `heights` (blocking client).
pub fn fetch_headers_at_heights_blocking(
    client: &esplora_client::BlockingClient,
    local_tip: CheckPoint<Header>,
    heights: impl IntoIterator<Item = u32>,
) -> Result<CheckPoint<Header>, Error> {
    let requested: BTreeSet<u32> = heights.into_iter().collect();
    let requested_vec: Vec<u32> = requested.iter().copied().collect();
    let missing: Vec<u32> = requested
        .iter()
        .copied()
        .filter(|h| local_tip.get(*h).is_none())
        .collect();

    let latest_blocks = client.get_block_infos(None)?;
    let tip_height = latest_blocks
        .iter()
        .map(|b| b.height)
        .max()
        .ok_or_else(|| Box::new(esplora_client::Error::HeaderHashNotFound(local_tip.hash())))?;

    let mut point_of_agreement = None;
    let mut conflicts = Vec::new();

    for local_cp in local_tip.iter() {
        let height = local_cp.height();
        if height > tip_height {
            continue;
        }
        let remote_header = fetch_header_at_height_blocking(client, height)?;
        if remote_header.block_hash() == local_cp.hash() {
            point_of_agreement = Some(local_cp);
            break;
        }
        conflicts.push((height, remote_header));
    }

    let agreement = match point_of_agreement {
        Some(cp) => cp,
        None => {
            return Err(Box::new(esplora_client::Error::HeaderHashNotFound(
                local_tip.hash(),
            )));
        }
    };

    let reorg_detected = !conflicts.is_empty();
    if missing.is_empty() && !reorg_detected {
        return Ok(local_tip);
    }

    let mut fetched = BTreeMap::new();
    for &height in &missing {
        if height > tip_height {
            return Err(Box::new(esplora_client::Error::HeaderHeightNotFound(
                height,
            )));
        }
        let header = fetch_header_at_height_blocking(client, height)?;
        fetched.insert(height, header);
    }

    let bridges = bridge_heights(&local_tip, agreement.height(), &requested_vec);
    Ok(build_fetch_headers_update(
        agreement, conflicts, fetched, &local_tip, &bridges,
    ))
}

fn fetch_header_at_height_blocking(
    client: &esplora_client::BlockingClient,
    height: u32,
) -> Result<Header, Error> {
    let hash = client.get_block_hash(height)?;
    Ok(client.get_header_by_hash(&hash)?)
}

#[cfg(feature = "async")]
pub mod async_impl {
    use super::*;

    use esplora_client::Sleeper;

    /// Fetch a reorg-aware [`CheckPoint<Header>`] update covering `heights` (async client).
    pub async fn fetch_headers_at_heights_async<S: Sleeper>(
        client: &esplora_client::AsyncClient<S>,
        local_tip: CheckPoint<Header>,
        heights: impl IntoIterator<Item = u32>,
    ) -> Result<CheckPoint<Header>, Error> {
        let requested: BTreeSet<u32> = heights.into_iter().collect();
        let requested_vec: Vec<u32> = requested.iter().copied().collect();
        let missing: Vec<u32> = requested
            .iter()
            .copied()
            .filter(|h| local_tip.get(*h).is_none())
            .collect();

        let latest_blocks = client.get_block_infos(None).await?;
        let tip_height = latest_blocks
            .iter()
            .map(|b| b.height)
            .max()
            .ok_or_else(|| Box::new(esplora_client::Error::HeaderHashNotFound(local_tip.hash())))?;

        let mut point_of_agreement = None;
        let mut conflicts = Vec::new();

        for local_cp in local_tip.iter() {
            let height = local_cp.height();
            if height > tip_height {
                continue;
            }
            let remote_header = fetch_header_at_height_async(client, height).await?;
            if remote_header.block_hash() == local_cp.hash() {
                point_of_agreement = Some(local_cp);
                break;
            }
            conflicts.push((height, remote_header));
        }

        let agreement = match point_of_agreement {
            Some(cp) => cp,
            None => {
                return Err(Box::new(esplora_client::Error::HeaderHashNotFound(
                    local_tip.hash(),
                )));
            }
        };

        let reorg_detected = !conflicts.is_empty();
        if missing.is_empty() && !reorg_detected {
            return Ok(local_tip);
        }

        let mut fetched = BTreeMap::new();
        for &height in &missing {
            if height > tip_height {
                return Err(Box::new(esplora_client::Error::HeaderHashNotFound(
                    local_tip.hash(),
                )));
            }
            let header = fetch_header_at_height_async(client, height).await?;
            fetched.insert(height, header);
        }

        let bridges = bridge_heights(&local_tip, agreement.height(), &requested_vec);
        Ok(build_fetch_headers_update(
            agreement, conflicts, fetched, &local_tip, &bridges,
        ))
    }

    async fn fetch_header_at_height_async<S: Sleeper>(
        client: &esplora_client::AsyncClient<S>,
        height: u32,
    ) -> Result<Header, Error> {
        let hash = client.get_block_hash(height).await?;
        Ok(client.get_header_by_hash(&hash).await?)
    }
}
