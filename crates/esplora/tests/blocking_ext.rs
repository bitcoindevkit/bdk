use bdk_chain::bitcoin::hashes::Hash;
use bdk_chain::local_chain::LocalChain;
use bdk_chain::BlockId;
use bdk_esplora::EsploraExt;
use electrsd::bitcoind::anyhow;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use esplora_client::{self, BlockHash, Builder};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use bdk_chain::bitcoin::{Address, Amount, Txid};
use bdk_testenv::TestEnv;

macro_rules! h {
    ($index:literal) => {{
        bdk_chain::bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

macro_rules! local_chain {
    [ $(($height:expr, $block_hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $block_hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
    }};
}

/// Ensure that update does not remove heights (from original), and all anchor heights are included.
#[test]
pub fn test_finalize_chain_update() -> anyhow::Result<()> {
    struct TestCase<'a> {
        name: &'a str,
        /// Initial blockchain height to start the env with.
        initial_env_height: u32,
        /// Initial checkpoint heights to start with.
        initial_cps: &'a [u32],
        /// The final blockchain height of the env.
        final_env_height: u32,
        /// The anchors to test with: `(height, txid)`. Only the height is provided as we can fetch
        /// the blockhash from the env.
        anchors: &'a [(u32, Txid)],
    }

    let test_cases = [
        TestCase {
            name: "chain_extends",
            initial_env_height: 60,
            initial_cps: &[59, 60],
            final_env_height: 90,
            anchors: &[],
        },
        TestCase {
            name: "introduce_older_heights",
            initial_env_height: 50,
            initial_cps: &[10, 15],
            final_env_height: 50,
            anchors: &[(11, h!("A")), (14, h!("B"))],
        },
        TestCase {
            name: "introduce_older_heights_after_chain_extends",
            initial_env_height: 50,
            initial_cps: &[10, 15],
            final_env_height: 100,
            anchors: &[(11, h!("A")), (14, h!("B"))],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        println!("[{}] running test case: {}", i, t.name);

        let env = TestEnv::new()?;
        let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_blocking()?;

        // set env to `initial_env_height`
        if let Some(to_mine) = t
            .initial_env_height
            .checked_sub(env.make_checkpoint_tip().height())
        {
            env.mine_blocks(to_mine as _, None)?;
        }
        while client.get_height()? < t.initial_env_height {
            std::thread::sleep(Duration::from_millis(10));
        }

        // craft initial `local_chain`
        let local_chain = {
            let (mut chain, _) = LocalChain::from_genesis_hash(env.genesis_hash()?);
            let chain_tip = chain.tip();
            let update_blocks = bdk_esplora::init_chain_update_blocking(&client, &chain_tip)?;
            let update_anchors = t
                .initial_cps
                .iter()
                .map(|&height| -> anyhow::Result<_> {
                    Ok((
                        BlockId {
                            height,
                            hash: env.bitcoind.client.get_block_hash(height as _)?,
                        },
                        Txid::all_zeros(),
                    ))
                })
                .collect::<anyhow::Result<BTreeSet<_>>>()?;
            let chain_update = bdk_esplora::finalize_chain_update_blocking(
                &client,
                &chain_tip,
                &update_anchors,
                update_blocks,
            )?;
            chain.apply_update(chain_update)?;
            chain
        };
        println!("local chain height: {}", local_chain.tip().height());

        // extend env chain
        if let Some(to_mine) = t
            .final_env_height
            .checked_sub(env.make_checkpoint_tip().height())
        {
            env.mine_blocks(to_mine as _, None)?;
        }
        while client.get_height()? < t.final_env_height {
            std::thread::sleep(Duration::from_millis(10));
        }

        // craft update
        let update = {
            let local_tip = local_chain.tip();
            let update_blocks = bdk_esplora::init_chain_update_blocking(&client, &local_tip)?;
            let update_anchors = t
                .anchors
                .iter()
                .map(|&(height, txid)| -> anyhow::Result<_> {
                    Ok((
                        BlockId {
                            height,
                            hash: env.bitcoind.client.get_block_hash(height as _)?,
                        },
                        txid,
                    ))
                })
                .collect::<anyhow::Result<_>>()?;
            bdk_esplora::finalize_chain_update_blocking(
                &client,
                &local_tip,
                &update_anchors,
                update_blocks,
            )?
        };

        // apply update
        let mut updated_local_chain = local_chain.clone();
        updated_local_chain.apply_update(update)?;
        println!(
            "updated local chain height: {}",
            updated_local_chain.tip().height()
        );

        assert!(
            {
                let initial_heights = local_chain
                    .iter_checkpoints()
                    .map(|cp| cp.height())
                    .collect::<BTreeSet<_>>();
                let updated_heights = updated_local_chain
                    .iter_checkpoints()
                    .map(|cp| cp.height())
                    .collect::<BTreeSet<_>>();
                updated_heights.is_superset(&initial_heights)
            },
            "heights from the initial chain must all be in the updated chain",
        );

        assert!(
            {
                let exp_anchor_heights = t
                    .anchors
                    .iter()
                    .map(|(h, _)| *h)
                    .chain(t.initial_cps.iter().copied())
                    .collect::<BTreeSet<_>>();
                let anchor_heights = updated_local_chain
                    .iter_checkpoints()
                    .map(|cp| cp.height())
                    .collect::<BTreeSet<_>>();
                anchor_heights.is_superset(&exp_anchor_heights)
            },
            "anchor heights must all be in updated chain",
        );
    }

    Ok(())
}

#[test]
pub fn test_update_tx_graph_without_keychain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_blocking();

    let receive_address0 =
        Address::from_str("bcrt1qc6fweuf4xjvz4x3gx3t9e0fh4hvqyu2qw4wvxm")?.assume_checked();
    let receive_address1 =
        Address::from_str("bcrt1qfjg5lv3dvc9az8patec8fjddrs4aqtauadnagr")?.assume_checked();

    let misc_spks = [
        receive_address0.script_pubkey(),
        receive_address1.script_pubkey(),
    ];

    let _block_hashes = env.mine_blocks(101, None)?;
    let txid1 = env.bitcoind.client.send_to_address(
        &receive_address1,
        Amount::from_sat(10000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let txid2 = env.bitcoind.client.send_to_address(
        &receive_address0,
        Amount::from_sat(20000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while client.get_height().unwrap() < 102 {
        sleep(Duration::from_millis(10))
    }

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    let sync_update = client.sync(
        cp_tip.clone(),
        misc_spks.into_iter(),
        vec![].into_iter(),
        vec![].into_iter(),
        1,
    )?;

    assert!(
        {
            let update_cps = sync_update
                .local_chain
                .tip
                .iter()
                .map(|cp| cp.block_id())
                .collect::<BTreeSet<_>>();
            let superset_cps = cp_tip
                .iter()
                .map(|cp| cp.block_id())
                .collect::<BTreeSet<_>>();
            superset_cps.is_superset(&update_cps)
        },
        "update should not alter original checkpoint tip since we already started with all checkpoints",
    );

    let graph_update = sync_update.tx_graph;
    // Check to see if we have the floating txouts available from our two created transactions'
    // previous outputs in order to calculate transaction fees.
    for tx in graph_update.full_txs() {
        // Retrieve the calculated fee from `TxGraph`, which will panic if we do not have the
        // floating txouts available from the transactions' previous outputs.
        let fee = graph_update.calculate_fee(&tx.tx).expect("Fee must exist");

        // Retrieve the fee in the transaction data from `bitcoind`.
        let tx_fee = env
            .bitcoind
            .client
            .get_transaction(&tx.txid, None)
            .expect("Tx must exist")
            .fee
            .expect("Fee must exist")
            .abs()
            .to_sat() as u64;

        // Check that the calculated fee matches the fee from the transaction data.
        assert_eq!(fee, tx_fee);
    }

    let mut graph_update_txids: Vec<Txid> = graph_update.full_txs().map(|tx| tx.txid).collect();
    graph_update_txids.sort();
    let mut expected_txids = vec![txid1, txid2];
    expected_txids.sort();
    assert_eq!(graph_update_txids, expected_txids);

    Ok(())
}

/// Test the bounds of the address scan depending on the `stop_gap`.
#[test]
pub fn test_update_tx_graph_stop_gap() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_blocking();
    let _block_hashes = env.mine_blocks(101, None)?;

    // Now let's test the gap limit. First of all get a chain of 10 addresses.
    let addresses = [
        "bcrt1qj9f7r8r3p2y0sqf4r3r62qysmkuh0fzep473d2ar7rcz64wqvhssjgf0z4",
        "bcrt1qmm5t0ch7vh2hryx9ctq3mswexcugqe4atkpkl2tetm8merqkthas3w7q30",
        "bcrt1qut9p7ej7l7lhyvekj28xknn8gnugtym4d5qvnp5shrsr4nksmfqsmyn87g",
        "bcrt1qqz0xtn3m235p2k96f5wa2dqukg6shxn9n3txe8arlrhjh5p744hsd957ww",
        "bcrt1q9c0t62a8l6wfytmf2t9lfj35avadk3mm8g4p3l84tp6rl66m48sqrme7wu",
        "bcrt1qkmh8yrk2v47cklt8dytk8f3ammcwa4q7dzattedzfhqzvfwwgyzsg59zrh",
        "bcrt1qvgrsrzy07gjkkfr5luplt0azxtfwmwq5t62gum5jr7zwcvep2acs8hhnp2",
        "bcrt1qw57edarcg50ansq8mk3guyrk78rk0fwvrds5xvqeupteu848zayq549av8",
        "bcrt1qvtve5ekf6e5kzs68knvnt2phfw6a0yjqrlgat392m6zt9jsvyxhqfx67ef",
        "bcrt1qw03ddumfs9z0kcu76ln7jrjfdwam20qtffmkcral3qtza90sp9kqm787uk",
    ];
    let addresses: Vec<_> = addresses
        .into_iter()
        .map(|s| Address::from_str(s).unwrap().assume_checked())
        .collect();
    let spks: Vec<_> = addresses
        .iter()
        .enumerate()
        .map(|(i, addr)| (i as u32, addr.script_pubkey()))
        .collect();
    let mut keychains = BTreeMap::new();
    keychains.insert(0, spks);

    // Then receive coins on the 4th address.
    let txid_4th_addr = env.bitcoind.client.send_to_address(
        &addresses[3],
        Amount::from_sat(10000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while client.get_height().unwrap() < 103 {
        sleep(Duration::from_millis(10))
    }

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    // A scan with a stop_gap of 3 won't find the transaction, but a scan with a gap limit of 4
    // will.
    let full_scan_update = client.full_scan(cp_tip.clone(), keychains.clone(), 3, 1)?;
    assert!(full_scan_update.tx_graph.full_txs().next().is_none());
    assert!(full_scan_update.last_active_indices.is_empty());
    let full_scan_update = client.full_scan(cp_tip.clone(), keychains.clone(), 4, 1)?;
    assert_eq!(
        full_scan_update.tx_graph.full_txs().next().unwrap().txid,
        txid_4th_addr
    );
    assert_eq!(full_scan_update.last_active_indices[&0], 3);

    // Now receive a coin on the last address.
    let txid_last_addr = env.bitcoind.client.send_to_address(
        &addresses[addresses.len() - 1],
        Amount::from_sat(10000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while client.get_height().unwrap() < 104 {
        sleep(Duration::from_millis(10))
    }

    // A scan with gap limit 5 won't find the second transaction, but a scan with gap limit 6 will.
    // The last active indice won't be updated in the first case but will in the second one.
    let full_scan_update = client.full_scan(cp_tip.clone(), keychains.clone(), 5, 1)?;
    let txs: HashSet<_> = full_scan_update
        .tx_graph
        .full_txs()
        .map(|tx| tx.txid)
        .collect();
    assert_eq!(txs.len(), 1);
    assert!(txs.contains(&txid_4th_addr));
    assert_eq!(full_scan_update.last_active_indices[&0], 3);
    let full_scan_update = client.full_scan(cp_tip.clone(), keychains, 6, 1)?;
    let txs: HashSet<_> = full_scan_update
        .tx_graph
        .full_txs()
        .map(|tx| tx.txid)
        .collect();
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&txid_4th_addr) && txs.contains(&txid_last_addr));
    assert_eq!(full_scan_update.last_active_indices[&0], 9);

    Ok(())
}

#[test]
fn update_local_chain() -> anyhow::Result<()> {
    const TIP_HEIGHT: u32 = 50;

    let env = TestEnv::new()?;
    let blocks = {
        let bitcoind_client = &env.bitcoind.client;
        assert_eq!(bitcoind_client.get_block_count()?, 1);
        [
            (0, bitcoind_client.get_block_hash(0)?),
            (1, bitcoind_client.get_block_hash(1)?),
        ]
        .into_iter()
        .chain((2..).zip(env.mine_blocks((TIP_HEIGHT - 1) as usize, None)?))
        .collect::<BTreeMap<_, _>>()
    };
    // so new blocks can be seen by Electrs
    let env = env.reset_electrsd()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_blocking();

    struct TestCase {
        name: &'static str,
        chain: LocalChain,
        request_heights: &'static [u32],
        exp_update_heights: &'static [u32],
    }

    let test_cases = [
        TestCase {
            name: "request_later_blocks",
            chain: local_chain![(0, blocks[&0]), (21, blocks[&21])],
            request_heights: &[22, 25, 28],
            exp_update_heights: &[21, 22, 25, 28],
        },
        TestCase {
            name: "request_prev_blocks",
            chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (5, blocks[&5])],
            request_heights: &[4],
            exp_update_heights: &[4, 5],
        },
        TestCase {
            name: "request_prev_blocks_2",
            chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (10, blocks[&10])],
            request_heights: &[4, 6],
            exp_update_heights: &[4, 6, 10],
        },
        TestCase {
            name: "request_later_and_prev_blocks",
            chain: local_chain![(0, blocks[&0]), (7, blocks[&7]), (11, blocks[&11])],
            request_heights: &[8, 9, 15],
            exp_update_heights: &[8, 9, 11, 15],
        },
        TestCase {
            name: "request_tip_only",
            chain: local_chain![(0, blocks[&0]), (5, blocks[&5]), (49, blocks[&49])],
            request_heights: &[TIP_HEIGHT],
            exp_update_heights: &[49],
        },
        TestCase {
            name: "request_nothing",
            chain: local_chain![(0, blocks[&0]), (13, blocks[&13]), (23, blocks[&23])],
            request_heights: &[],
            exp_update_heights: &[23],
        },
        TestCase {
            name: "request_nothing_during_reorg",
            chain: local_chain![(0, blocks[&0]), (13, blocks[&13]), (23, h!("23"))],
            request_heights: &[],
            exp_update_heights: &[13, 23],
        },
        TestCase {
            name: "request_nothing_during_reorg_2",
            chain: local_chain![
                (0, blocks[&0]),
                (21, blocks[&21]),
                (22, h!("22")),
                (23, h!("23"))
            ],
            request_heights: &[],
            exp_update_heights: &[21, 22, 23],
        },
        TestCase {
            name: "request_prev_blocks_during_reorg",
            chain: local_chain![
                (0, blocks[&0]),
                (21, blocks[&21]),
                (22, h!("22")),
                (23, h!("23"))
            ],
            request_heights: &[17, 20],
            exp_update_heights: &[17, 20, 21, 22, 23],
        },
        TestCase {
            name: "request_later_blocks_during_reorg",
            chain: local_chain![
                (0, blocks[&0]),
                (9, blocks[&9]),
                (22, h!("22")),
                (23, h!("23"))
            ],
            request_heights: &[25, 27],
            exp_update_heights: &[9, 22, 23, 25, 27],
        },
        TestCase {
            name: "request_later_blocks_during_reorg_2",
            chain: local_chain![(0, blocks[&0]), (9, h!("9"))],
            request_heights: &[10],
            exp_update_heights: &[0, 9, 10],
        },
        TestCase {
            name: "request_later_and_prev_blocks_during_reorg",
            chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (9, h!("9"))],
            request_heights: &[8, 11],
            exp_update_heights: &[1, 8, 9, 11],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        println!("Case {}: {}", i, t.name);
        let mut chain = t.chain;
        let cp_tip = chain.tip();

        let new_blocks =
            bdk_esplora::init_chain_update_blocking(&client, &cp_tip).map_err(|err| {
                anyhow::format_err!("[{}:{}] `init_chain_update` failed: {}", i, t.name, err)
            })?;

        let mock_anchors = t
            .request_heights
            .iter()
            .map(|&h| {
                let anchor_blockhash: BlockHash = bdk_chain::bitcoin::hashes::Hash::hash(
                    &format!("hash_at_height_{}", h).into_bytes(),
                );
                let txid: Txid = bdk_chain::bitcoin::hashes::Hash::hash(
                    &format!("txid_at_height_{}", h).into_bytes(),
                );
                let anchor = BlockId {
                    height: h,
                    hash: anchor_blockhash,
                };
                (anchor, txid)
            })
            .collect::<BTreeSet<_>>();

        let chain_update = bdk_esplora::finalize_chain_update_blocking(
            &client,
            &cp_tip,
            &mock_anchors,
            new_blocks,
        )?;
        let update_blocks = chain_update
            .tip
            .iter()
            .map(|cp| cp.block_id())
            .collect::<BTreeSet<_>>();

        let exp_update_blocks = t
            .exp_update_heights
            .iter()
            .map(|&height| {
                let hash = blocks[&height];
                BlockId { height, hash }
            })
            .chain(
                // Electrs Esplora `get_block` call fetches 10 blocks which is included in the
                // update
                blocks
                    .range(TIP_HEIGHT - 9..)
                    .map(|(&height, &hash)| BlockId { height, hash }),
            )
            .collect::<BTreeSet<_>>();

        assert!(
            update_blocks.is_superset(&exp_update_blocks),
            "[{}:{}] unexpected update",
            i,
            t.name
        );

        let _ = chain
            .apply_update(chain_update)
            .unwrap_or_else(|err| panic!("[{}:{}] update failed to apply: {}", i, t.name, err));

        // all requested heights must exist in the final chain
        for height in t.request_heights {
            let exp_blockhash = blocks.get(height).expect("block must exist in bitcoind");
            assert_eq!(
                chain.get(*height).map(|cp| cp.hash()),
                Some(*exp_blockhash),
                "[{}:{}] block {}:{} must exist in final chain",
                i,
                t.name,
                height,
                exp_blockhash
            );
        }
    }

    Ok(())
}
