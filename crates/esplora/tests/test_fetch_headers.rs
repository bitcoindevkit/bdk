use std::collections::BTreeMap;

use bdk_chain::local_chain::{ChangeSet, LocalChain};
use bdk_core::bitcoin::block::Header;
use bdk_esplora::EsploraExt;
use bdk_testenv::{anyhow, TestEnv};
use esplora_client::Builder;

fn header_at(client: &esplora_client::BlockingClient, height: u32) -> anyhow::Result<Header> {
    let hash = client.get_block_hash(height)?;
    Ok(client.get_header_by_hash(&hash)?)
}

fn local_chain_from_headers(blocks: &[(u32, Header)]) -> LocalChain<Header> {
    let map: BTreeMap<u32, Header> = blocks.iter().copied().collect();
    LocalChain::from_blocks(map).expect("valid sparse chain")
}

fn esplora_client(env: &TestEnv) -> anyhow::Result<esplora_client::BlockingClient> {
    let base_url = format!("http://{}", env.electrsd.esplora_url.clone().unwrap());
    Ok(Builder::new(base_url.as_str()).build_blocking())
}

#[test]
fn fetch_headers_at_heights_exact_heights() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(30, None)?;
    let env = env.reset_electrsd()?;
    let client = esplora_client(&env)?;

    let genesis = header_at(&client, 0)?;
    let h21 = header_at(&client, 21)?;
    let mut chain = local_chain_from_headers(&[(0, genesis), (21, h21)]);

    let update = client.fetch_headers_at_heights(chain.tip(), [22, 25, 28])?;

    for h in [22, 25, 28] {
        assert!(update.get(h).is_some(), "update must contain height {h}");
    }

    chain.apply_update(update)?;
    Ok(())
}

#[test]
fn fetch_headers_at_heights_skip_already_known() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(10, None)?;
    let env = env.reset_electrsd()?;
    let client = esplora_client(&env)?;

    let blocks: Vec<(u32, Header)> = (0..=5)
        .map(|h| Ok((h, header_at(&client, h)?)))
        .collect::<anyhow::Result<_>>()?;
    let mut chain = local_chain_from_headers(&blocks);

    let update = client.fetch_headers_at_heights(chain.tip(), [3, 4, 5, 6])?;
    let changeset = chain.apply_update(update)?;
    assert!(changeset.blocks.contains_key(&6));
    assert_eq!(changeset.blocks.get(&3), None);

    Ok(())
}

#[test]
fn fetch_headers_at_heights_no_op_empty_changeset() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(5, None)?;
    let env = env.reset_electrsd()?;
    let client = esplora_client(&env)?;

    let blocks: Vec<(u32, Header)> = (0..=3)
        .map(|h| Ok((h, header_at(&client, h)?)))
        .collect::<anyhow::Result<_>>()?;
    let chain = local_chain_from_headers(&blocks);

    let update = client.fetch_headers_at_heights(chain.tip(), [1, 2, 3])?;
    assert_eq!(update, chain.tip());

    let mut chain = chain;
    let changeset = chain.apply_update(update)?;
    assert_eq!(changeset, ChangeSet::default());

    Ok(())
}

#[test]
fn fetch_headers_at_heights_mtp_recovery() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(30, None)?;
    let env = env.reset_electrsd()?;
    let client = esplora_client(&env)?;

    let genesis = header_at(&client, 0)?;
    let h21 = header_at(&client, 21)?;
    let mut chain = local_chain_from_headers(&[(0, genesis), (21, h21)]);

    assert_eq!(chain.get(21).unwrap().median_time_past(), None);

    let update = client.fetch_headers_at_heights(chain.tip(), (11..=21).collect::<Vec<_>>())?;
    chain.apply_update(update)?;
    assert!(chain.get(21).unwrap().median_time_past().is_some());

    Ok(())
}

#[test]
fn fetch_headers_at_heights_sparse_gap_bridge() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(10, None)?;
    let env = env.reset_electrsd()?;
    let client = esplora_client(&env)?;

    let blocks = [
        (0, header_at(&client, 0)?),
        (1, header_at(&client, 1)?),
        (5, header_at(&client, 5)?),
    ];
    let mut chain = local_chain_from_headers(&blocks);

    let update = client.fetch_headers_at_heights(chain.tip(), [4])?;
    assert!(update.get(4).is_some());
    assert!(update.get(5).is_some());

    chain.apply_update(update)?;
    assert_eq!(chain.get(4).map(|cp| cp.height()), Some(4));

    Ok(())
}

#[test]
fn fetch_headers_at_heights_no_tip_suffix_padding() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(50, None)?;
    let env = env.reset_electrsd()?;
    let client = esplora_client(&env)?;

    let genesis = header_at(&client, 0)?;
    let h21 = header_at(&client, 21)?;
    let chain = local_chain_from_headers(&[(0, genesis), (21, h21)]);

    let update = client.fetch_headers_at_heights(chain.tip(), [22])?;
    let heights: BTreeMap<u32, _> = update.iter().map(|cp| (cp.height(), ())).collect();

    // Must not include the electrs tip-suffix (~10 blocks) beyond what connects height 22.
    assert!(heights.contains_key(&22));
    assert!(!heights.contains_key(&50));

    Ok(())
}
