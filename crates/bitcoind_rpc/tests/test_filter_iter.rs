use bdk_bitcoind_rpc::bip158::{Error, FilterIter};
use bdk_core::{BlockId, CheckPoint};
use bdk_testenv::{anyhow, bitcoind, TestEnv};
use bitcoin::{Address, Amount, Network, ScriptBuf};
use bitcoincore_rpc::RpcApi;

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
fn filter_iter_matches_blocks() -> anyhow::Result<()> {
    let env = testenv()?;
    let addr = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();

    let _ = env.mine_blocks(100, Some(addr.clone()))?;
    assert_eq!(env.rpc_client().get_block_count()?, 101);

    // Send tx to external address to confirm at height = 102
    let _txid = env.send(
        &Address::from_script(
            &ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?,
            Network::Regtest,
        )?,
        Amount::from_btc(0.42)?,
    )?;
    let _ = env.mine_blocks(1, None);

    let genesis_hash = env.genesis_hash()?;
    let cp = CheckPoint::new(BlockId {
        height: 0,
        hash: genesis_hash,
    });

    let iter = FilterIter::new(&env.bitcoind.client, cp, [addr.script_pubkey()]);

    for res in iter {
        let event = res?;
        let height = event.height();
        if (2..102).contains(&height) {
            assert!(event.is_match(), "expected to match height {height}");
        }
    }

    Ok(())
}

#[test]
fn filter_iter_error_wrong_network() -> anyhow::Result<()> {
    let env = testenv()?;
    let _ = env.mine_blocks(10, None)?;

    // Try to initialize FilterIter with a CP on the wrong network
    let block_id = BlockId {
        height: 0,
        hash: bitcoin::hashes::Hash::hash(b"wrong-hash"),
    };
    let cp = CheckPoint::new(block_id);
    let mut iter = FilterIter::new(&env.bitcoind.client, cp, [ScriptBuf::new()]);
    assert!(matches!(iter.next(), Some(Err(Error::ReorgDepthExceeded))));

    Ok(())
}

// Test that while a reorg is detected we delay incrementing the best height
#[test]
fn filter_iter_detects_reorgs() -> anyhow::Result<()> {
    const MINE_TO: u32 = 16;

    let env = testenv()?;
    let rpc = env.rpc_client();
    while rpc.get_block_count()? < MINE_TO as u64 {
        let _ = env.mine_blocks(1, None)?;
    }

    let genesis_hash = env.genesis_hash()?;
    let cp = CheckPoint::new(BlockId {
        height: 0,
        hash: genesis_hash,
    });

    let spk = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;
    let mut iter = FilterIter::new(&env.bitcoind.client, cp, [spk]);

    // Process events to height (MINE_TO - 1)
    loop {
        if iter.next().unwrap()?.height() == MINE_TO - 1 {
            break;
        }
    }

    for _ in 0..3 {
        // Invalidate and remine 1 block
        let _ = env.reorg(1)?;

        // Call next. If we detect a reorg, we'll see no change in the event height
        assert_eq!(iter.next().unwrap()?.height(), MINE_TO - 1);
    }

    // If no reorg, then height should increment normally from here on
    assert_eq!(iter.next().unwrap()?.height(), MINE_TO);
    assert!(iter.next().is_none());

    Ok(())
}
