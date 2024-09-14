use bitcoin::{constants, Address, Amount, Network, ScriptBuf};

use bdk_bitcoind_rpc::bip158::FilterIter;
use bdk_core::{BlockId, CheckPoint};
use bdk_testenv::{anyhow, bitcoind, block_id, TestEnv};
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
        let update_cp = iter.chain_update().unwrap();
        let mut update_blocks: Vec<_> = update_cp.iter().map(|cp| cp.block_id()).collect();
        update_blocks.reverse();
        assert_eq!(update_blocks, test.exp, "{}", test.name);
    });

    Ok(())
}

#[test]
fn filter_iter_returns_matched_blocks() -> anyhow::Result<()> {
    use bdk_bitcoind_rpc::bip158::{Event, EventInner};
    let env = testenv()?;
    let rpc = env.rpc_client();
    while rpc.get_block_count()? < 101 {
        let _ = env.mine_blocks(1, None)?;
    }

    // send tx
    let spk = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;
    let txid = env.send(
        &Address::from_script(&spk, Network::Regtest)?,
        Amount::from_btc(0.42)?,
    )?;
    let _ = env.mine_blocks(1, None);

    // match blocks
    let mut iter = FilterIter::new_with_height(rpc, 1);
    iter.add_spk(spk);
    assert_eq!(iter.get_tip()?.unwrap().height, 102);

    for res in iter {
        let event = res?;
        match event {
            event if event.height() <= 101 => assert!(!event.is_match()),
            Event::Block(EventInner { height, block }) => {
                assert_eq!(height, 102);
                assert!(block.txdata.iter().any(|tx| tx.compute_txid() == txid));
            }
            Event::NoMatch(_) => panic!("expected to match block 102"),
        }
    }

    Ok(())
}

#[test]
fn filter_iter_error_no_scripts() -> anyhow::Result<()> {
    use bdk_bitcoind_rpc::bip158::Error;
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
