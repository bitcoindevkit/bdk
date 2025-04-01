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

#[test]
fn filter_iter_handles_reorg_during_iteration() -> anyhow::Result<()> {
    let env = testenv()?;
    let client = env.rpc_client();

    // 1. Initial setup & mining
    println!("STEP: Initial mining (target height 101 for maturity)");

    let additional_blocks_to_mine = 100; // Mine 100 *additional* blocks
    let _ = env.mine_blocks(additional_blocks_to_mine, None)?;
    // *****************************
    // Now check against the expected final height
    let expected_initial_height = 101; // Assuming genesis(0)+initial(1)+100 = 101
    assert_eq!(
        client.get_block_count()?,
        expected_initial_height as u64,
        "Block count should be {} after initial mine",
        expected_initial_height
    );

    // 2. Create watched script
    println!("STEP: Creating watched script");
    // ***** FIX: Ensure address and spk_to_watch are defined here *****
    let address = client
        .get_new_address(
            Some("reorg-test"),
            Some(bitcoincore_rpc::json::AddressType::Bech32),
        )?
        .assume_checked();
    let spk_to_watch = address.script_pubkey();
    // ******************************************************************
    println!("Watching SPK: {}", spk_to_watch.to_hex_string());

    // 3. Mine block A (now height 102) WITH relevant tx
    println!("STEP: Sending tx for original block A (now height 102)");
    let txid_a = env.send(&address, Amount::from_sat(1000))?;
    println!("STEP: Mining original block A (now height 102)");
    let hash_102a = env.mine_blocks(1, None)?[0];
    println!("Block 102 (A) hash: {}", hash_102a);
    assert_eq!(
        client.get_block_count()?,
        102,
        "Block count should be 102 after mining block A"
    );

    // 4. Mine block B (now height 103) WITH relevant tx
    println!("STEP: Sending tx for original block B (now height 103)");
    let txid_b = env.send(&address, Amount::from_sat(2000))?;
    println!("STEP: Mining original block B (now height 103)");
    let hash_103b = env.mine_blocks(1, None)?[0];
    println!("Block 103 (B) hash: {}", hash_103b);
    assert_eq!(
        client.get_block_count()?,
        103,
        "Block count should be 103 after mining block B"
    );

    // 5. Instantiate FilterIter (Start height remains 101)
    println!("STEP: Instantiating FilterIter");
    let start_height = 101; // Start scan *after* the initial mine tip (height 101)
    let mut iter = FilterIter::new_with_height(client, start_height + 1); // Start processing from height 102
    iter.add_spk(spk_to_watch.clone());
    let initial_tip = iter.get_tip()?.expect("Should get initial tip");
    assert_eq!(initial_tip.height, 103); // Tip is now block B
    assert_eq!(initial_tip.hash, hash_103b);

    // 6. Iterate once (process block A - now height 102)
    println!("STEP: Iterating once (original block A at height 102)");
    let event_a_result = iter.next().expect("Iterator should have item A");
    let event_a = event_a_result?;
    println!("First event: {:?}", event_a);
    match &event_a {
        Event::Block(EventInner { height, block }) => {
            assert_eq!(*height, 102);
            assert_eq!(block.block_hash(), hash_102a);
            assert!(block.txdata.iter().any(|tx| tx.compute_txid() == txid_a));
        }
        _ => panic!("Expected block A"),
    }

    // 7. Simulate Reorg (Invalidate blocks 103B and 102A)
    println!("STEP: Invalidating blocks B (103) and A (102)");
    println!(
        "Invalidating blocks B ({}) and A ({})",
        hash_103b, hash_102a
    );
    client.invalidate_block(&hash_103b)?;
    client.invalidate_block(&hash_102a)?;
    // Current tip is now 101

    // 8. Mine Replacement Blocks WITHOUT relevant txs
    // Block A' (height 102) - empty or unrelated txs
    println!("STEP: Mining replacement block A' (height 102, no send)");
    println!("Mining replacement Block 102 (A') without relevant tx");
    let hash_102a_prime = env.mine_blocks(1, None)?[0]; // Mine block 102 (A')
    println!("Block 102 (A') hash: {}", hash_102a_prime);
    assert_eq!(client.get_block_count()?, 102);
    assert_ne!(hash_102a, hash_102a_prime);

    // Block B' (height 103) - empty or unrelated txs
    println!("STEP: Mining replacement block B' (height 103, no send)");
    println!("Mining replacement Block 103 (B') without relevant tx");
    let hash_103b_prime = env.mine_blocks(1, None)?[0]; // Mine block 103 (B')
    println!("Block 103 (B') hash: {}", hash_103b_prime);
    assert_eq!(client.get_block_count()?, 103);
    assert_ne!(hash_103b, hash_103b_prime);

    // 9. Continue Iterating & Collect Events AFTER reorg
    // Iterator should now process heights 102 (A') and 103 (B').
    let mut post_reorg_events: Vec<bdk_bitcoind_rpc::bip158::Event> = Vec::new();

    println!("STEP: Starting post-reorg iteration loop");
    println!("Continuing iteration after reorg...");
    while let Some(event_result) = iter.next() {
        match event_result {
            Ok(event) => {
                println!("Post-reorg event: {:?}", event);
                post_reorg_events.push(event);
            }
            Err(e) => {
                // Print the error and fail the test immediately if an error occurs during iteration
                eprintln!("Error during post-reorg iteration: {:?}", e);
                return Err(anyhow::Error::msg(format!(
                    "Iteration failed post-reorg: {:?}",
                    e
                )));
            }
        }
    }

    // 10. Assertions (Adjust heights)
    println!("STEP: Checking post-reorg assertions");
    println!("Checking assertions (expecting failure)...");

    // Check event for height 102 post-reorg (Block A')
    let event_102_post = post_reorg_events.iter().find(|e| e.height() == 102);
    assert!(
        event_102_post.is_some(),
        "Should have yielded an event for height 102 post-reorg (Block A')"
    );
    match event_102_post.unwrap() {
        Event::Block(inner) => {
            assert_eq!(
                inner.block.block_hash(),
                hash_102a_prime,
                "BUG: Iterator yielded wrong block for height 102! Expected A', maybe got A?"
            );
        }
        Event::NoMatch(h) => {
            assert_eq!(*h, 102, "Should be NoMatch for height 102");
        }
    }

    // Check event for height 103 post-reorg (Block B')
    let event_103_post = post_reorg_events.iter().find(|e| e.height() == 103);
    assert!(
        event_103_post.is_some(),
        "Should have yielded an event for height 103 post-reorg (Block B')"
    );
    match event_103_post.unwrap() {
        Event::Block(inner) => {
            assert_eq!(
                inner.block.block_hash(),
                hash_103b_prime,
                "BUG: Iterator yielded wrong block for height 103! Expected B', maybe got B?"
            );
            assert!(
                !inner
                    .block
                    .txdata
                    .iter()
                    .any(|tx| tx.compute_txid() == txid_b),
                "BUG: Iterator yielded block for height 103 containing old txid_b!"
            );
        }
        Event::NoMatch(h) => {
            assert_eq!(*h, 103, "Should be NoMatch for height 103");
        }
    }

    // Check chain update tip (Adjust height)
    println!("STEP: Checking chain_update");
    let final_update = iter.chain_update();
    assert!(final_update.is_some(), "Should get a final chain update");
    let final_tip_id = final_update.unwrap().block_id();
    assert_eq!(
        final_tip_id.height, 103,
        "BUG: Final checkpoint height mismatch!"
    );
    assert_eq!(
        final_tip_id.hash, hash_103b_prime,
        "BUG: Final checkpoint hash mismatch! Expected hash of B'."
    );

    println!("If you see this, the test PASSED unexpectedly. The bug might already be fixed or the test logic is flawed.");

    Ok(())
}
