use bdk_bitcoind_rpc::bip158::{Event, EventInner, FilterIter};
use bdk_core::{BlockId, CheckPoint};
use bdk_testenv::{anyhow, bitcoind, block_id, TestEnv};
use bitcoin::{constants, Address, Amount, Network, ScriptBuf};
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
fn filter_iter_returns_matched_blocks() -> anyhow::Result<()> {
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

#[test]
#[allow(clippy::print_stdout)]
fn filter_iter_handles_reorg() -> anyhow::Result<()> {
    let env = testenv()?;
    let client = env.rpc_client();

    // 1. Initial setup & mining
    println!("STEP: Initial mining (target height 102 for maturity)");

    let expected_initial_height = 102;
    while env.rpc_client().get_block_count()? < expected_initial_height {
        let _ = env.mine_blocks(1, None)?;
    }
    // *****************************
    // Check the expected initial height
    assert_eq!(
        client.get_block_count()?,
        expected_initial_height,
        "Block count should be {} after initial mine",
        expected_initial_height
    );

    // 2. Create watched script
    println!("STEP: Creating watched script");
    // Ensure address and spk_to_watch are defined here *****
    // ******************************************************************
    let spk_to_watch = ScriptBuf::from_hex("0014446906a6560d8ad760db3156706e72e171f3a2aa")?;
    let address = Address::from_script(&spk_to_watch, Network::Regtest)?;
    println!("Watching SPK: {}", spk_to_watch.to_hex_string());

    // Create 2 txs to be confirmed at consecutive heights.
    // We have to choose our UTXOs now to make sure one doesn't get invalidated
    // later by a reorg.
    let unspent = client.list_unspent(None, None, None, None, None)?;
    assert!(unspent.len() >= 2);
    use bdk_testenv::bitcoincore_rpc::bitcoincore_rpc_json::CreateRawTransactionInput;
    let unspent_1 = &unspent[0];
    let unspent_2 = &unspent[1];
    let utxo_1 = CreateRawTransactionInput {
        txid: unspent_1.txid,
        vout: unspent_1.vout,
        sequence: None,
    };
    let utxo_2 = CreateRawTransactionInput {
        txid: unspent_2.txid,
        vout: unspent_2.vout,
        sequence: None,
    };

    // create tx 1
    println!("STEP: Creating transactions to send");
    let to_send = Amount::ONE_BTC;
    let fee = Amount::from_sat(1_000);
    let change_addr = client.get_new_address(None, None)?.assume_checked();
    let change_amt = unspent_1.amount - to_send - fee;
    let out = [
        (address.to_string(), to_send),
        (change_addr.to_string(), change_amt),
    ]
    .into();
    let to_send = Amount::ONE_BTC * 2;
    let tx = client.create_raw_transaction(&[utxo_1], &out, None, None)?;
    let res = client.sign_raw_transaction_with_wallet(&tx, None, None)?;
    let tx_1 = res.transaction()?;
    // create tx 2
    let change_addr = client.get_new_address(None, None)?.assume_checked();
    let change_amt = unspent_2.amount - to_send - fee;
    let out = [
        (address.to_string(), to_send),
        (change_addr.to_string(), change_amt),
    ]
    .into();
    let tx = client.create_raw_transaction(&[utxo_2], &out, None, None)?;
    let res = client.sign_raw_transaction_with_wallet(&tx, None, None)?;
    let tx_2 = res.transaction()?;

    // let mine_to: u32 = 103;

    println!("STEP: Mining to height {}", 103);
    while env.rpc_client().get_block_count()? < 103 {
        let _ = env.mine_blocks(1, None)?;
    }

    // 3. Mine block A WITH relevant tx
    println!("STEP: Sending tx for original block A");
    let txid_a = client.send_raw_transaction(&tx_1)?;
    println!("STEP: Mining original block A");
    let hash_104 = env.mine_blocks(1, None)?[0];

    // 4. Mine block B WITH relevant tx 2
    println!("STEP: Sending tx 2 for original block B");
    let txid_b = client.send_raw_transaction(&tx_2)?;
    println!("STEP: Mining original block B");
    let hash_105 = env.mine_blocks(1, None)?[0];

    assert_eq!(
        client.get_block_count()?,
        105,
        "Block count should be 105 after mining block B"
    );

    // 5. Instantiate FilterIter at start height 104
    println!("STEP: Instantiating FilterIter");
    // Start processing from height 104
    let start_height = 104;
    let mut iter = FilterIter::new_with_height(client, start_height);
    iter.add_spk(spk_to_watch.clone());
    let initial_tip = iter.get_tip()?.expect("Should get initial tip");
    assert_eq!(initial_tip.height, 105);
    assert_eq!(initial_tip.hash, hash_105);

    // 6. Iterate once processing block A
    println!("STEP: Iterating once (original block A)");
    let event_a = iter.next().expect("Iterator should have item A")?;
    // println!("First event: {:?}", event_a);
    match event_a {
        Event::Block(EventInner { height, block }) => {
            assert_eq!(height, 104);
            assert_eq!(block.block_hash(), hash_104);
            assert!(block.txdata.iter().any(|tx| tx.compute_txid() == txid_a));
        }
        _ => panic!("Expected relevant tx at block A 102"),
    }

    // 7. Simulate Reorg (Invalidate blocks B and A)
    println!("STEP: Invalidating original blocks B and A");
    println!("Invalidating blocks B ({}) and A ({})", hash_105, hash_104);
    client.invalidate_block(&hash_105)?;
    client.invalidate_block(&hash_104)?;
    // We should see 2 unconfirmed txs in mempool
    let raw_mempool = client.get_raw_mempool()?;
    assert_eq!(raw_mempool.len(), 2);
    println!(
        "{} txs in mempool at height {}",
        raw_mempool.len(),
        client.get_block_count()?
    );

    // 8. Mine Replacement Blocks WITH relevant txs
    // First mine Block A'
    println!("STEP: Mining replacement block A' (with send tx x2)");
    let hash_104_prime = env.mine_blocks(1, None)?[0];
    let height = client.get_block_count()?;
    println!("Block {} (A') hash: {}", height, hash_104_prime);
    assert_eq!(height, 104);
    assert_ne!(hash_104, hash_104_prime);

    // Mine Block B' - empty or unrelated txs
    println!("STEP: Mining replacement block B' (no send tx)");
    let hash_105_prime = env.mine_blocks(1, None)?[0];
    let height = client.get_block_count()?;
    println!("Block {} (B') hash: {}", height, hash_105_prime);
    assert_eq!(height, 105);
    assert_ne!(hash_105, hash_105_prime);

    // 9. Continue Iterating & Collect Events AFTER reorg
    // Iterator should now process heights 109 (A') and 110 (B').
    let mut post_reorg_events: Vec<Event> = vec![];

    println!("STEP: Starting post-reorg iteration loop");
    println!("Continuing iteration after reorg...");
    for event_result in iter.by_ref() {
        let event = event_result?;
        println!(
            "Post-reorg event height: {}, matched: {}",
            event.height(),
            event.is_match(),
        );
        post_reorg_events.push(event);
    }

    // 10. Assertions
    println!("STEP: Checking post-reorg assertions");

    // Check for event post-reorg (Block A')
    let event_104_post = post_reorg_events.iter().find(|e| e.height() == 104);
    assert!(
        event_104_post.is_some(),
        "Should have yielded an event for post-reorg (Block A')"
    );
    match event_104_post.unwrap() {
        Event::Block(inner) => {
            assert_eq!(
                inner.block.block_hash(),
                hash_104_prime,
                "BUG: Iterator yielded wrong block for height 104! Expected A'"
            );
            assert!(
                inner
                    .block
                    .txdata
                    .iter()
                    .any(|tx| tx.compute_txid() == txid_a),
                "Expected relevant tx A"
            );
            assert!(
                inner
                    .block
                    .txdata
                    .iter()
                    .any(|tx| tx.compute_txid() == txid_b),
                "Expected relevant tx B"
            );
        }
        Event::NoMatch(..) => {
            panic!("Expected to match height 104");
        }
    }

    // Check for event post-reorg (Block B')
    let event_105_post = post_reorg_events.iter().find(|e| e.height() == 105);
    assert!(
        event_105_post.is_some(),
        "Should have yielded an event for post-reorg (Block B')"
    );
    match event_105_post.unwrap() {
        Event::NoMatch(h) => {
            assert_eq!(*h, 105, "Should be NoMatch for block B'");
        }
        Event::Block(..) => {
            panic!("Expected NoMatch for block B'");
        }
    }

    // Check chain update tip
    // println!("STEP: Checking chain_update");
    let final_update = iter.chain_update();
    assert!(
        final_update.is_none(),
        "We didn't instantiate FilterIter with a checkpoint"
    );

    Ok(())
}

// Test that while a reorg is detected we delay incrementing the best height
#[test]
#[ignore]
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
