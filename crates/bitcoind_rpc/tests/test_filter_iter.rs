use bdk_bitcoind_rpc::bip158::{Error, FilterIter};
use bdk_core::{BlockId, CheckPoint};
use bdk_testenv::{anyhow, bitcoind, block_id, TestEnv};
use bitcoin::{constants, Network, ScriptBuf};
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

// Test the result of `chain_update` given a local checkpoint.
//
// new blocks
//       2--3--4--5--6--7--8--9--10--11
//
// case 1: base below new blocks
// 0-
// case 2: base overlaps with new blocks
// 0--1--2--3--4
// case 3: stale tip (with overlap)
// 0--1--2--3--4--x
// case 4: stale tip (no overlap)
// 0--x
#[test]
fn get_tip_and_chain_update() -> anyhow::Result<()> {
    let env = testenv()?;

    let genesis_hash = constants::genesis_block(Network::Regtest).block_hash();
    let genesis = BlockId {
        height: 0,
        hash: genesis_hash,
    };

    let hash = env.rpc_client().get_best_block_hash()?;
    let header = env.rpc_client().get_block_header_info(&hash)?;
    assert_eq!(header.height, 1);
    let block_1 = BlockId {
        height: header.height as u32,
        hash,
    };

    // `FilterIter` will try to return up to ten recent blocks
    // so we keep them for reference
    let new_blocks: Vec<BlockId> = (2..12)
        .zip(env.mine_blocks(10, None)?)
        .map(BlockId::from)
        .collect();

    let new_tip = *new_blocks.last().unwrap();

    struct TestCase {
        // name
        name: &'static str,
        // local blocks
        chain: Vec<BlockId>,
        // expected blocks
        exp: Vec<BlockId>,
    }

    // For each test we create a new `FilterIter` with the checkpoint given
    // by the blocks in the test chain. Then we sync to the remote tip and
    // check the blocks that are returned in the chain update.
    [
        TestCase {
            name: "point of agreement below new blocks, expect base + new",
            chain: vec![genesis, block_1],
            exp: [block_1].into_iter().chain(new_blocks.clone()).collect(),
        },
        TestCase {
            name: "point of agreement genesis, expect base + new",
            chain: vec![genesis],
            exp: [genesis].into_iter().chain(new_blocks.clone()).collect(),
        },
        TestCase {
            name: "point of agreement within new blocks, expect base + remaining",
            chain: new_blocks[..=2].to_vec(),
            exp: new_blocks[2..].to_vec(),
        },
        TestCase {
            name: "stale tip within new blocks, expect base + corrected + remaining",
            // base height: 4, stale height: 5
            chain: vec![new_blocks[2], block_id!(5, "E")],
            exp: new_blocks[2..].to_vec(),
        },
        TestCase {
            name: "stale tip below new blocks, expect base + corrected + new",
            chain: vec![genesis, block_id!(1, "A")],
            exp: [genesis, block_1].into_iter().chain(new_blocks).collect(),
        },
    ]
    .into_iter()
    .for_each(|test| {
        let cp = CheckPoint::from_block_ids(test.chain).unwrap();
        let mut iter = FilterIter::new_with_checkpoint(env.rpc_client(), cp);
        assert_eq!(iter.get_tip().unwrap(), Some(new_tip));
        for _res in iter.by_ref() {}
        let update_cp = iter.chain_update().unwrap();
        let mut update_blocks: Vec<_> = update_cp.iter().map(|cp| cp.block_id()).collect();
        update_blocks.reverse();
        assert_eq!(update_blocks, test.exp, "{}", test.name);
    });

    Ok(())
}

#[test]
fn filter_iter_error_no_scripts() -> anyhow::Result<()> {
    let env = testenv()?;
    let _ = env.mine_blocks(2, None)?;

    let mut iter = FilterIter::new_with_height(env.rpc_client(), 1);
    assert_eq!(iter.get_tip()?.unwrap().height, 3);

    // iterator should return three errors
    for _ in 0..3 {
        assert!(matches!(iter.next().unwrap(), Err(Error::NoScripts)));
    }
    assert!(iter.next().is_none());

    Ok(())
}

// Test that while a reorg is detected we delay incrementing the best height
#[test]
fn repeat_reorgs() -> anyhow::Result<()> {
    const MINE_TO: u32 = 11;

    let env = testenv()?;
    let rpc = env.rpc_client();
    while rpc.get_block_count()? < MINE_TO as u64 {
        let _ = env.mine_blocks(1, None)?;
    }

    let spk = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;

    let mut iter = FilterIter::new_with_height(env.rpc_client(), 1);
    iter.add_spk(spk);
    assert_eq!(iter.get_tip()?.unwrap().height, MINE_TO);

    // Process events to height (MINE_TO - 1)
    loop {
        if iter.next().unwrap()?.height() == MINE_TO - 1 {
            break;
        }
    }

    for _ in 0..3 {
        // Invalidate 2 blocks and remine to height = MINE_TO
        let _ = env.reorg(2)?;

        // Call next. If we detect a reorg, we'll see no change in the event height
        assert_eq!(iter.next().unwrap()?.height(), MINE_TO - 1);
    }

    // If no reorg, then height should increment normally from here on
    assert_eq!(iter.next().unwrap()?.height(), MINE_TO);
    assert!(iter.next().is_none());

    Ok(())
}

#[test]
fn filter_iter_error_wrong_network() -> anyhow::Result<()> {
    let env = testenv()?;
    let rpc = env.rpc_client();
    let _ = env.mine_blocks(10, None)?;

    // Try to initialize FilterIter with a CP on the wrong network
    let block_id = BlockId {
        height: 0,
        hash: bitcoin::hashes::Hash::hash(b"wrong-hash"),
    };
    let cp = CheckPoint::new(block_id);
    let mut iter = FilterIter::new_with_checkpoint(rpc, cp);
    let err = iter
        .get_tip()
        .expect_err("`get_tip` should fail to find PoA");
    assert!(matches!(err, Error::ReorgDepthExceeded));

    Ok(())
}

#[test]
fn filter_iter_max_reorg_depth() -> anyhow::Result<()> {
    use bdk_bitcoind_rpc::bip158::Error;

    let env = testenv()?;
    let client = env.rpc_client();

    const BASE_HEIGHT: u32 = 10;
    const REORG_LEN: u32 = 101;
    const STOP_HEIGHT: u32 = BASE_HEIGHT + REORG_LEN;

    while client.get_block_count()? < STOP_HEIGHT as u64 {
        env.mine_blocks(1, None)?;
    }

    let mut iter = FilterIter::new_with_height(client, BASE_HEIGHT);
    let spk = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;
    iter.add_spk(spk.clone());
    assert_eq!(iter.get_tip()?.unwrap().height, STOP_HEIGHT);

    // Consume events up to final height - 1.
    loop {
        if iter.next().unwrap()?.height() == STOP_HEIGHT - 1 {
            break;
        }
    }

    let _ = env.reorg(REORG_LEN as usize)?;

    assert!(matches!(iter.next(), Some(Err(Error::ReorgDepthExceeded))));

    Ok(())
}
