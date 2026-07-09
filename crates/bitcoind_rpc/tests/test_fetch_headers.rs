use std::collections::BTreeMap;

use bdk_bitcoind_rpc::{fetch_headers_at_heights, FetchHeadersError};
use bdk_chain::local_chain::{ChangeSet, LocalChain};
use bdk_testenv::{anyhow, TestEnv};
use bitcoin::block::Header;

use crate::common::ClientExt;

mod common;

fn header_at(env: &TestEnv, height: u32) -> anyhow::Result<Header> {
    let hash = env.get_block_hash(height as u64)?;
    Ok(env.rpc_client().get_block_header(&hash)?.into_model()?.0)
}

fn local_chain_from_headers(blocks: &[(u32, Header)]) -> LocalChain<Header> {
    let map: BTreeMap<u32, Header> = blocks.iter().copied().collect();
    LocalChain::from_blocks(map).expect("valid sparse chain")
}

#[test]
fn fetch_headers_at_heights_exact_heights() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(30, None)?;

    let genesis = header_at(&env, 0)?;
    let h21 = header_at(&env, 21)?;
    let mut chain = local_chain_from_headers(&[(0, genesis), (21, h21)]);

    let update =
        fetch_headers_at_heights(&ClientExt::get_rpc_client(&env)?, chain.tip(), [22, 25, 28])?;

    for h in [22, 25, 28] {
        assert!(update.get(h).is_some(), "update must contain height {h}");
    }

    chain.apply_update(update)?;
    for h in [22, 25, 28] {
        assert_eq!(chain.get(h).map(|cp| cp.height()), Some(h));
    }

    Ok(())
}

#[test]
fn fetch_headers_at_heights_skip_already_known() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(10, None)?;

    let blocks: Vec<(u32, Header)> = (0..=5)
        .map(|h| Ok((h, header_at(&env, h)?)))
        .collect::<anyhow::Result<_>>()?;
    let mut chain = local_chain_from_headers(&blocks);

    let update =
        fetch_headers_at_heights(&ClientExt::get_rpc_client(&env)?, chain.tip(), [3, 4, 5, 6])?;

    assert!(update.get(6).is_some());

    let changeset = chain.apply_update(update)?;
    assert_eq!(changeset.blocks.get(&3), None);
    assert_eq!(changeset.blocks.get(&4), None);
    assert_eq!(changeset.blocks.get(&5), None);
    assert!(changeset.blocks.contains_key(&6));

    Ok(())
}

#[test]
fn fetch_headers_at_heights_no_op_empty_changeset() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(5, None)?;

    let blocks: Vec<(u32, Header)> = (0..=3)
        .map(|h| Ok((h, header_at(&env, h)?)))
        .collect::<anyhow::Result<_>>()?;
    let chain = local_chain_from_headers(&blocks);

    let update =
        fetch_headers_at_heights(&ClientExt::get_rpc_client(&env)?, chain.tip(), [1, 2, 3])?;

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

    let genesis = header_at(&env, 0)?;
    let h21 = header_at(&env, 21)?;
    let mut chain = local_chain_from_headers(&[(0, genesis), (21, h21)]);

    assert_eq!(chain.get(21).unwrap().median_time_past(), None);

    let missing: Vec<u32> = (11..=21).collect();
    let update = fetch_headers_at_heights(&ClientExt::get_rpc_client(&env)?, chain.tip(), missing)?;

    chain.apply_update(update)?;
    assert!(chain.get(21).unwrap().median_time_past().is_some());

    Ok(())
}

#[test]
fn fetch_headers_at_heights_sparse_gap_bridge() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(10, None)?;

    let blocks = [
        (0, header_at(&env, 0)?),
        (1, header_at(&env, 1)?),
        (5, header_at(&env, 5)?),
    ];
    let mut chain = local_chain_from_headers(&blocks);

    let update = fetch_headers_at_heights(&ClientExt::get_rpc_client(&env)?, chain.tip(), [4])?;

    assert!(update.get(4).is_some());
    assert!(update.get(5).is_some());

    chain.apply_update(update)?;
    assert_eq!(chain.get(4).map(|cp| cp.height()), Some(4));

    Ok(())
}

#[test]
fn fetch_headers_at_heights_height_above_tip_errors() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(5, None)?;

    let genesis = header_at(&env, 0)?;
    let chain = local_chain_from_headers(&[(0, genesis)]);
    let tip = env.rpc_client().get_block_count()?.into_model().0 as u32;

    let err = fetch_headers_at_heights(&ClientExt::get_rpc_client(&env)?, chain.tip(), [tip + 100])
        .unwrap_err();

    assert!(matches!(err, FetchHeadersError::Rpc(_)));

    Ok(())
}
