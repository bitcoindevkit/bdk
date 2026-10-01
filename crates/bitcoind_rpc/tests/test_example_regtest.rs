// End-to-end regtest coverage for `examples/filter_iter.rs` (issue #1618).
//
// This mirrors what the example does: build a [`FilterIter`] from the genesis
// checkpoint over a watched script, mine regtest blocks, fund the watched
// script, and confirm the filter matches only the funding height. It runs in
// CI on every push/PR via the `regtest-bitcoind-rpc-example` job.
//
// Lessons from closed PR #2143 (unmerged, Copilot-only review) applied here:
// - use `env.bitcoind.rpc_url()` via `ClientExt` (no hand-formatted rpc url),
// - `require_network(Network::Regtest)` for address validation,
// - `Amount::from_sat` (no `from_btc(..).unwrap()`),
// - typed amount/height asserts (no `contains("5000000")` string checks).
use bdk_bitcoind_rpc::bip158::FilterIter;
use bdk_core::CheckPoint;
use bdk_testenv::{anyhow, bitcoind, TestEnv};
use bitcoin::{Amount, Network};
use bitcoincore_rpc::RpcApi;

use crate::common::ClientExt;

mod common;

/// Regtest env with compact block filters enabled (required by `FilterIter`).
fn testenv() -> anyhow::Result<TestEnv> {
    let mut conf = bitcoind::Conf::default();
    conf.args.push("-blockfilterindex=1");
    conf.args.push("-peerblockfilters=1");
    TestEnv::new_with_config(bdk_testenv::Config {
        bitcoind: conf,
        ..Default::default()
    })
}

#[test]
fn example_filter_iter_regtest_end_to_end() -> anyhow::Result<()> {
    const EXPECTED_SATS: u64 = 5_000_000;
    let env = testenv()?;
    let rpc = ClientExt::get_rpc_client(&env)?;

    // Watched script: fresh regtest address, explicitly network-checked.
    let watched_addr = rpc
        .get_new_address(None, None)?
        .require_network(Network::Regtest)?;
    let watched_spk = watched_addr.script_pubkey();

    // Mature the chain so the funding tx can confirm (coinbase maturity).
    let miner_addr = rpc.get_new_address(None, None)?.assume_checked();
    let _ = env.mine_blocks(101, Some(miner_addr))?;
    let tip_before_fund = rpc.get_block_count()?;

    // Fund the watched script (typed amount, no float BTC conversion).
    let fund_amount = Amount::from_sat(EXPECTED_SATS);
    let fund_txid = env.send(&watched_addr, fund_amount)?;

    // Unconfirmed funds are visible with minconf=0 (typed, no string match).
    let unconfirmed = rpc.get_received_by_address(&watched_addr, Some(0))?;
    assert_eq!(
        unconfirmed, fund_amount,
        "unconfirmed received should be {EXPECTED_SATS} sats"
    );

    // Confirm the funding tx.
    let _ = env.mine_blocks(1, None)?;
    let tip_after_fund = rpc.get_block_count()?;
    assert_eq!(tip_after_fund, tip_before_fund + 1);

    // Confirmed balance is visible with minconf=1.
    let confirmed = rpc.get_received_by_address(&watched_addr, Some(1))?;
    assert_eq!(
        confirmed, fund_amount,
        "confirmed received should be {EXPECTED_SATS} sats"
    );

    // Drive the example's `FilterIter` path from genesis over the watched spk.
    let genesis_hash = env.genesis_hash()?;
    let cp = CheckPoint::new(0, genesis_hash);
    let client = ClientExt::get_rpc_client(&env)?;
    let iter = FilterIter::new(&client, cp, [watched_spk]);

    let mut matched_heights = Vec::new();
    let mut seen_tip = 0_u32;
    for res in iter {
        let event = res?;
        seen_tip = event.height();
        if event.is_match() {
            matched_heights.push(event.height());
        } else {
            assert!(
                event.height() != tip_after_fund as u32,
                "funding height must match the filter"
            );
        }
    }

    assert_eq!(seen_tip, tip_after_fund as u32, "iter must reach the tip");
    assert!(
        matched_heights.contains(&(tip_after_fund as u32)),
        "filter must match funding height {tip_after_fund}, got {matched_heights:?} (txid {fund_txid})"
    );
    assert!(
        !matched_heights.iter().any(|&h| h < tip_after_fund as u32),
        "no pre-fund block may match, got {matched_heights:?}"
    );

    Ok(())
}
