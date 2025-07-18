use std::{collections::BTreeSet, ops::Deref};

use bdk_bitcoind_rpc::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_chain::{
    bitcoin::{Address, Amount, Txid},
    local_chain::{CheckPoint, LocalChain},
    spk_txout::SpkTxOutIndex,
    Balance, BlockId, CanonicalizationParams, IndexedTxGraph, Merge,
};
use bdk_testenv::{anyhow, TestEnv};
use bitcoin::{hashes::Hash, Block, Network, OutPoint, ScriptBuf, WScriptHash};
use bitcoincore_rpc::RpcApi;

/// Ensure that blocks are emitted in order even after reorg.
///
/// 1. Mine 101 blocks.
/// 2. Emit blocks from [`Emitter`] and update the [`LocalChain`].
/// 3. Reorg highest 6 blocks.
/// 4. Emit blocks from [`Emitter`] and re-update the [`LocalChain`].
#[test]
pub fn test_sync_local_chain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let network_tip = env.rpc_client().get_block_count()?;
    let (mut local_chain, _) = LocalChain::from_genesis_hash(env.rpc_client().get_block_hash(0)?);
    let mut emitter = Emitter::new(
        env.rpc_client(),
        local_chain.tip(),
        0,
        NO_EXPECTED_MEMPOOL_TXS,
    );

    // Mine some blocks and return the actual block hashes.
    // Because initializing `ElectrsD` already mines some blocks, we must include those too when
    // returning block hashes.
    let exp_hashes = {
        let mut hashes = (0..=network_tip)
            .map(|height| env.rpc_client().get_block_hash(height))
            .collect::<Result<Vec<_>, _>>()?;
        hashes.extend(env.mine_blocks(101 - network_tip as usize, None)?);
        hashes
    };

    // See if the emitter outputs the right blocks.

    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        let hash = emission.block_hash();
        assert_eq!(
            emission.block_hash(),
            exp_hashes[height as usize],
            "emitted block hash is unexpected"
        );

        assert_eq!(
            local_chain.apply_update(emission.checkpoint,)?,
            [(height, Some(hash))].into(),
            "chain update changeset is unexpected",
        );
    }

    assert_eq!(
        local_chain
            .iter_checkpoints()
            .map(|cp| (cp.height(), cp.hash()))
            .collect::<BTreeSet<_>>(),
        exp_hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| (i as u32, *hash))
            .collect::<BTreeSet<_>>(),
        "final local_chain state is unexpected",
    );

    // Perform reorg.
    let reorged_blocks = env.reorg(6)?;
    let exp_hashes = exp_hashes
        .iter()
        .take(exp_hashes.len() - reorged_blocks.len())
        .chain(&reorged_blocks)
        .cloned()
        .collect::<Vec<_>>();

    // See if the emitter outputs the right blocks.

    let mut exp_height = exp_hashes.len() - reorged_blocks.len();
    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        let hash = emission.block_hash();
        assert_eq!(
            height, exp_height as u32,
            "emitted block has unexpected height"
        );

        assert_eq!(
            hash, exp_hashes[height as usize],
            "emitted block is unexpected"
        );

        assert_eq!(
            local_chain.apply_update(emission.checkpoint,)?,
            if exp_height == exp_hashes.len() - reorged_blocks.len() {
                bdk_chain::local_chain::ChangeSet {
                    blocks: core::iter::once((height, Some(hash)))
                        .chain((height + 1..exp_hashes.len() as u32).map(|h| (h, None)))
                        .collect(),
                }
            } else {
                [(height, Some(hash))].into()
            },
            "chain update changeset is unexpected",
        );

        exp_height += 1;
    }

    assert_eq!(
        local_chain
            .iter_checkpoints()
            .map(|cp| (cp.height(), cp.hash()))
            .collect::<BTreeSet<_>>(),
        exp_hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| (i as u32, *hash))
            .collect::<BTreeSet<_>>(),
        "final local_chain state is unexpected after reorg",
    );

    Ok(())
}

/// Ensure that [`EmittedUpdate::into_tx_graph_update`] behaves appropriately for both mempool and
/// block updates.
///
/// [`EmittedUpdate::into_tx_graph_update`]: bdk_bitcoind_rpc::EmittedUpdate::into_tx_graph_update
#[test]
fn test_into_tx_graph() -> anyhow::Result<()> {
    let env = TestEnv::new()?;

    let addr_0 = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();
    let addr_1 = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();
    let addr_2 = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();

    env.mine_blocks(101, None)?;

    let (mut chain, _) = LocalChain::from_genesis_hash(env.rpc_client().get_block_hash(0)?);
    let mut indexed_tx_graph = IndexedTxGraph::<BlockId, _>::new({
        let mut index = SpkTxOutIndex::<usize>::default();
        index.insert_spk(0, addr_0.script_pubkey());
        index.insert_spk(1, addr_1.script_pubkey());
        index.insert_spk(2, addr_2.script_pubkey());
        index
    });

    let emitter = &mut Emitter::new(env.rpc_client(), chain.tip(), 0, NO_EXPECTED_MEMPOOL_TXS);

    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        let _ = chain.apply_update(emission.checkpoint)?;
        let indexed_additions = indexed_tx_graph.apply_block_relevant(&emission.block, height);
        assert!(indexed_additions.is_empty());
    }

    // send 3 txs to a tracked address, these txs will be in the mempool
    let exp_txids = {
        let mut txids = BTreeSet::new();
        for _ in 0..3 {
            txids.insert(env.rpc_client().send_to_address(
                &addr_0,
                Amount::from_sat(10_000),
                None,
                None,
                None,
                None,
                None,
                None,
            )?);
        }
        txids
    };

    // expect that the next block should be none and we should get 3 txs from mempool
    {
        // next block should be `None`
        assert!(emitter.next_block()?.is_none());

        let mempool_txs = emitter.mempool()?;
        let indexed_additions = indexed_tx_graph.batch_insert_unconfirmed(mempool_txs.update);
        assert_eq!(
            indexed_additions
                .tx_graph
                .txs
                .iter()
                .map(|tx| tx.compute_txid())
                .collect::<BTreeSet<Txid>>(),
            exp_txids,
            "changeset should have the 3 mempool transactions",
        );
        assert!(indexed_additions.tx_graph.anchors.is_empty());
    }

    // mine a block that confirms the 3 txs
    let exp_block_hash = env.mine_blocks(1, None)?[0];
    let exp_block_height = env.rpc_client().get_block_info(&exp_block_hash)?.height as u32;
    let exp_anchors = exp_txids
        .iter()
        .map({
            let anchor = BlockId {
                height: exp_block_height,
                hash: exp_block_hash,
            };
            move |&txid| (anchor, txid)
        })
        .collect::<BTreeSet<_>>();

    // must receive mined block which will confirm the transactions.
    {
        let emission = emitter.next_block()?.expect("must get mined block");
        let height = emission.block_height();
        let _ = chain.apply_update(emission.checkpoint)?;
        let indexed_additions = indexed_tx_graph.apply_block_relevant(&emission.block, height);
        assert!(indexed_additions.tx_graph.txs.is_empty());
        assert!(indexed_additions.tx_graph.txouts.is_empty());
        assert_eq!(indexed_additions.tx_graph.anchors, exp_anchors);
    }

    Ok(())
}

/// Ensure next block emitted after reorg is at reorg height.
///
/// After a reorg, if the last-emitted block height is equal or greater than the reorg height, and
/// the fallback height is equal to or lower than the reorg height, the next block/header emission
/// should be at the reorg height.
///
/// TODO: If the reorg height is lower than the fallback height, how do we find a block height to
/// emit that can connect with our receiver chain?
#[test]
fn ensure_block_emitted_after_reorg_is_at_reorg_height() -> anyhow::Result<()> {
    const EMITTER_START_HEIGHT: usize = 100;
    const CHAIN_TIP_HEIGHT: usize = 110;

    let env = TestEnv::new()?;
    let mut emitter = Emitter::new(
        env.rpc_client(),
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.rpc_client().get_block_hash(0)?,
        }),
        EMITTER_START_HEIGHT as _,
        NO_EXPECTED_MEMPOOL_TXS,
    );

    env.mine_blocks(CHAIN_TIP_HEIGHT, None)?;
    while emitter.next_block()?.is_some() {}

    for reorg_count in 1..=10 {
        let replaced_blocks = env.reorg_empty_blocks(reorg_count)?;
        let next_emission = emitter.next_block()?.expect("must emit block after reorg");
        assert_eq!(
            (
                next_emission.block_height() as usize,
                next_emission.block_hash()
            ),
            replaced_blocks[0],
            "block emitted after reorg should be at the reorg height"
        );
        while emitter.next_block()?.is_some() {}
    }

    Ok(())
}

fn process_block(
    recv_chain: &mut LocalChain,
    recv_graph: &mut IndexedTxGraph<BlockId, SpkTxOutIndex<()>>,
    block: Block,
    block_height: u32,
) -> anyhow::Result<()> {
    recv_chain.apply_update(CheckPoint::from_header(&block.header, block_height))?;
    let _ = recv_graph.apply_block(block, block_height);
    Ok(())
}

fn sync_from_emitter<C>(
    recv_chain: &mut LocalChain,
    recv_graph: &mut IndexedTxGraph<BlockId, SpkTxOutIndex<()>>,
    emitter: &mut Emitter<C>,
) -> anyhow::Result<()>
where
    C: Deref,
    C::Target: bitcoincore_rpc::RpcApi,
{
    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        process_block(recv_chain, recv_graph, emission.block, height)?;
    }
    Ok(())
}

fn get_balance(
    recv_chain: &LocalChain,
    recv_graph: &IndexedTxGraph<BlockId, SpkTxOutIndex<()>>,
) -> anyhow::Result<Balance> {
    let chain_tip = recv_chain.tip().block_id();
    let outpoints = recv_graph.index.outpoints().clone();
    let balance = recv_graph.graph().balance(
        recv_chain,
        chain_tip,
        CanonicalizationParams::default(),
        outpoints,
        |_, _| true,
    );
    Ok(balance)
}

/// If a block is reorged out, ensure that containing transactions that do not exist in the
/// replacement block(s) become unconfirmed.
#[test]
fn tx_can_become_unconfirmed_after_reorg() -> anyhow::Result<()> {
    const PREMINE_COUNT: usize = 101;
    const ADDITIONAL_COUNT: usize = 11;
    const SEND_AMOUNT: Amount = Amount::from_sat(10_000);

    let env = TestEnv::new()?;
    let mut emitter = Emitter::new(
        env.rpc_client(),
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.rpc_client().get_block_hash(0)?,
        }),
        0,
        NO_EXPECTED_MEMPOOL_TXS,
    );

    // setup addresses
    let addr_to_mine = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();
    let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, Network::Regtest)?;

    // setup receiver
    let (mut recv_chain, _) = LocalChain::from_genesis_hash(env.rpc_client().get_block_hash(0)?);
    let mut recv_graph = IndexedTxGraph::<BlockId, _>::new({
        let mut recv_index = SpkTxOutIndex::default();
        recv_index.insert_spk((), spk_to_track.clone());
        recv_index
    });

    // mine and sync receiver up to tip
    env.mine_blocks(PREMINE_COUNT, Some(addr_to_mine))?;

    // create transactions that are tracked by our receiver
    for _ in 0..ADDITIONAL_COUNT {
        let txid = env.send(&addr_to_track, SEND_AMOUNT)?;

        // lock outputs that send to `addr_to_track`
        let outpoints_to_lock = env
            .rpc_client()
            .get_transaction(&txid, None)?
            .transaction()?
            .output
            .into_iter()
            .enumerate()
            .filter(|(_, txo)| txo.script_pubkey == spk_to_track)
            .map(|(vout, _)| OutPoint::new(txid, vout as _))
            .collect::<Vec<_>>();
        env.rpc_client().lock_unspent(&outpoints_to_lock)?;

        let _ = env.mine_blocks(1, None)?;
    }

    // get emitter up to tip
    sync_from_emitter(&mut recv_chain, &mut recv_graph, &mut emitter)?;

    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            confirmed: SEND_AMOUNT * ADDITIONAL_COUNT as u64,
            ..Balance::default()
        },
        "initial balance must be correct",
    );

    // perform reorgs with different depths
    for reorg_count in 1..=ADDITIONAL_COUNT {
        env.reorg_empty_blocks(reorg_count)?;
        sync_from_emitter(&mut recv_chain, &mut recv_graph, &mut emitter)?;

        assert_eq!(
            get_balance(&recv_chain, &recv_graph)?,
            Balance {
                trusted_pending: SEND_AMOUNT * reorg_count as u64,
                confirmed: SEND_AMOUNT * (ADDITIONAL_COUNT - reorg_count) as u64,
                ..Balance::default()
            },
            "reorg_count: {reorg_count}",
        );
    }

    Ok(())
}

/// Ensure avoid-re-emission-logic is sound when [`Emitter`] is synced to tip.
///
/// The receiver (bdk_chain structures) is synced to the chain tip, and there is txs in the mempool.
/// When we call Emitter::mempool multiple times, mempool txs should not be re-emitted, even if the
/// chain tip is extended.
#[test]
fn mempool_avoids_re_emission() -> anyhow::Result<()> {
    const BLOCKS_TO_MINE: usize = 101;
    const MEMPOOL_TX_COUNT: usize = 2;

    let env = TestEnv::new()?;
    let mut emitter = Emitter::new(
        env.rpc_client(),
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.rpc_client().get_block_hash(0)?,
        }),
        0,
        NO_EXPECTED_MEMPOOL_TXS,
    );

    // mine blocks and sync up emitter
    let addr = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();
    env.mine_blocks(BLOCKS_TO_MINE, Some(addr.clone()))?;
    while emitter.next_block()?.is_some() {}

    // have some random txs in mempool
    let exp_txids = (0..MEMPOOL_TX_COUNT)
        .map(|_| env.send(&addr, Amount::from_sat(2100)))
        .collect::<Result<BTreeSet<Txid>, _>>()?;

    // First two emissions should include all transactions.
    for _ in 0..2 {
        let emitted_txids = emitter
            .mempool()?
            .update
            .into_iter()
            .map(|(tx, _)| tx.compute_txid())
            .collect::<BTreeSet<Txid>>();
        assert_eq!(
            emitted_txids, exp_txids,
            "all mempool txs should be emitted"
        );
    }

    // mine empty blocks + sync up our emitter -> we should still not re-emit
    for _ in 0..BLOCKS_TO_MINE {
        env.mine_empty_block()?;
    }
    while emitter.next_block()?.is_some() {}
    let emitted_txids = emitter
        .mempool()?
        .update
        .into_iter()
        .map(|(tx, _)| tx.compute_txid())
        .collect::<BTreeSet<Txid>>();
    assert_eq!(
        emitted_txids, exp_txids,
        "all mempool txs should be emitted"
    );

    Ok(())
}

/// If blockchain re-org includes the start height, emit new start height block
///
/// 1. mine 101 blocks
/// 2. emit blocks 99a, 100a
/// 3. invalidate blocks 99a, 100a, 101a
/// 4. mine new blocks 99b, 100b, 101b
/// 5. emit block 99b
///
/// The block hash of 99b should be different than 99a, but their previous block hashes should
/// be the same.
#[test]
fn no_agreement_point() -> anyhow::Result<()> {
    const PREMINE_COUNT: usize = 101;

    let env = TestEnv::new()?;

    // start height is 99
    let mut emitter = Emitter::new(
        env.rpc_client(),
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.rpc_client().get_block_hash(0)?,
        }),
        (PREMINE_COUNT - 2) as u32,
        NO_EXPECTED_MEMPOOL_TXS,
    );

    // mine 101 blocks
    env.mine_blocks(PREMINE_COUNT, None)?;

    // emit block 99a
    let block_header_99a = emitter
        .next_block()?
        .expect("block 99a header")
        .block
        .header;
    let block_hash_99a = block_header_99a.block_hash();
    let block_hash_98a = block_header_99a.prev_blockhash;

    // emit block 100a
    let block_header_100a = emitter.next_block()?.expect("block 100a header").block;
    let block_hash_100a = block_header_100a.block_hash();

    // get hash for block 101a
    let block_hash_101a = env.rpc_client().get_block_hash(101)?;

    // invalidate blocks 99a, 100a, 101a
    env.rpc_client().invalidate_block(&block_hash_99a)?;
    env.rpc_client().invalidate_block(&block_hash_100a)?;
    env.rpc_client().invalidate_block(&block_hash_101a)?;

    // mine new blocks 99b, 100b, 101b
    env.mine_blocks(3, None)?;

    // emit block header 99b
    let block_header_99b = emitter
        .next_block()?
        .expect("block 99b header")
        .block
        .header;
    let block_hash_99b = block_header_99b.block_hash();
    let block_hash_98b = block_header_99b.prev_blockhash;

    assert_ne!(block_hash_99a, block_hash_99b);
    assert_eq!(block_hash_98a, block_hash_98b);

    Ok(())
}

/// Validates that when an unconfirmed transaction is double-spent (and thus evicted from the
/// mempool), the emitter reports it in `evicted_txids`, and after inserting that eviction into the
/// graph it no longer appears in the set of canonical transactions.
///
/// 1. Broadcast a first tx (tx1) and confirm it arrives in unconfirmed set.
/// 2. Double-spend tx1 with tx1b and verify `mempool()` reports tx1 as evicted.
/// 3. Insert the eviction into the graph and assert tx1 is no longer canonical.
#[test]
fn test_expect_tx_evicted() -> anyhow::Result<()> {
    use bdk_bitcoind_rpc::bitcoincore_rpc::bitcoin;
    use bdk_bitcoind_rpc::bitcoincore_rpc::bitcoincore_rpc_json::CreateRawTransactionInput;
    use bdk_chain::miniscript;
    use bdk_chain::spk_txout::SpkTxOutIndex;
    use bitcoin::constants::genesis_block;
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::Network;
    use std::collections::HashMap;
    let env = TestEnv::new()?;

    let s = bdk_testenv::utils::DESCRIPTORS[0];
    let desc = miniscript::Descriptor::parse_descriptor(&Secp256k1::new(), s)
        .unwrap()
        .0;
    let spk = desc.at_derivation_index(0)?.script_pubkey();

    let mut chain = LocalChain::from_genesis_hash(genesis_block(Network::Regtest).block_hash()).0;
    let chain_tip = chain.tip().block_id();

    let mut index = SpkTxOutIndex::default();
    index.insert_spk((), spk.clone());
    let mut graph = IndexedTxGraph::<BlockId, _>::new(index);

    // Receive tx1.
    let _ = env.mine_blocks(100, None)?;
    let txid_1 = env.send(
        &Address::from_script(&spk, Network::Regtest)?,
        Amount::ONE_BTC,
    )?;
    let tx_1 = env
        .rpc_client()
        .get_transaction(&txid_1, None)?
        .transaction()?;

    let mut emitter = Emitter::new(env.rpc_client(), chain.tip(), 1, core::iter::once(tx_1));
    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        chain.apply_update(CheckPoint::from_header(&emission.block.header, height))?;
    }

    let changeset = graph.batch_insert_unconfirmed(emitter.mempool()?.update);
    assert!(changeset
        .tx_graph
        .txs
        .iter()
        .any(|tx| tx.compute_txid() == txid_1));

    // Double spend tx1.

    // Get `prevout` from core.
    let core = env.rpc_client();
    let tx1 = &core.get_raw_transaction(&txid_1, None)?;
    let txin = &tx1.input[0];
    let op = txin.previous_output;

    // Create `tx1b` using the previous output from tx1.
    let utxo = CreateRawTransactionInput {
        txid: op.txid,
        vout: op.vout,
        sequence: None,
    };
    let addr = core.get_new_address(None, None)?.assume_checked();
    let tx = core.create_raw_transaction(
        &[utxo],
        &HashMap::from([(addr.to_string(), Amount::from_btc(49.99)?)]),
        None,
        None,
    )?;
    let res = core.sign_raw_transaction_with_wallet(&tx, None, None)?;
    let tx1b = res.transaction()?;

    // Send the tx.
    let _txid_2 = core.send_raw_transaction(&tx1b)?;

    // Retrieve the expected unconfirmed txids and spks from the graph.
    let exp_spk_txids = graph
        .list_expected_spk_txids(&chain, chain_tip, ..)
        .collect::<Vec<_>>();
    assert_eq!(exp_spk_txids, vec![(spk, txid_1)]);

    // Check that mempool emission contains evicted txid.
    let mempool_event = emitter.mempool()?;
    assert!(mempool_event
        .evicted
        .iter()
        .any(|(txid, _)| txid == &txid_1));

    // Update graph with evicted tx.
    let _ = graph.batch_insert_relevant_evicted_at(mempool_event.evicted);

    let canonical_txids = graph
        .graph()
        .list_canonical_txs(&chain, chain_tip, CanonicalizationParams::default())
        .map(|tx| tx.tx_node.compute_txid())
        .collect::<Vec<_>>();
    // tx1 should no longer be canonical.
    assert!(!canonical_txids.contains(&txid_1));

    Ok(())
}

#[test]
fn detect_new_mempool_txs() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    env.mine_blocks(101, None)?;

    let addr = env
        .rpc_client()
        .get_new_address(None, None)?
        .require_network(Network::Regtest)?;

    let mut emitter = Emitter::new(
        env.rpc_client(),
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.rpc_client().get_block_hash(0)?,
        }),
        0,
        NO_EXPECTED_MEMPOOL_TXS,
    );

    while emitter.next_block()?.is_some() {}

    for n in 0..5 {
        let txid = env.send(&addr, Amount::ONE_BTC)?;
        let new_txs = emitter.mempool()?.update;
        assert!(
            new_txs.iter().any(|(tx, _)| tx.compute_txid() == txid),
            "must detect new tx {n}"
        );
    }

    Ok(())
}
