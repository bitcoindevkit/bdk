use std::collections::BTreeSet;
use std::sync::Arc;

use bdk_bitcoind_rpc::{Emitter, EmitterError};
use bdk_chain::{
    bitcoin::{Address, Amount, Transaction, Txid},
    local_chain::{CheckPoint, LocalChain},
    spk_txout::SpkTxOutIndex,
    Balance, BlockId, IndexedTxGraph, Merge,
};
use bdk_testenv::{
    anyhow,
    bitcoind::{Input, Output},
    TestEnv,
};
use bitcoin::{hashes::Hash, Block, Network, ScriptBuf, WScriptHash};

use crate::common::ClientExt;

mod common;

/// Ensures blocks are emitted consecutively with correct hashes, and that after a reorg the
/// emitter re-emits the replacement blocks at the same heights with updated hashes.
///
/// 1. Mine 101 blocks.
/// 2. Emit blocks from [`Emitter`] and update the [`LocalChain`].
/// 3. Reorg highest 6 blocks.
/// 4. Emit blocks from [`Emitter`] and re-update the [`LocalChain`].
#[test]
pub fn blocks_emitted_in_order_and_after_reorg() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let network_tip = env.rpc_client().get_block_count()?.into_model().0;
    let (mut local_chain, _) = LocalChain::from_genesis(env.genesis_hash()?);

    let client = ClientExt::get_rpc_client(&env)?;
    let mut emitter = Emitter::new(
        &client,
        local_chain.tip(),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // Mine some blocks and return the actual block hashes.
    // Because initializing `TestEnv` already mines some blocks, we must include those too when
    // returning block hashes.
    let exp_hashes = {
        let mut hashes = (0..=network_tip)
            .map(|height| env.get_block_hash(height))
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

/// Verifies the mempool → confirmation pipeline: unconfirmed transactions appear in
/// [`Emitter::mempool`] and receive block anchors once mined.
///
/// 1. Mine 101 blocks and sync emitter to tip.
/// 2. Send 3 transactions to a tracked address — they will be in the mempool.
/// 3. Assert `next_block` returns `None` (at tip) and `mempool` returns all 3 txs.
/// 4. Mine a block confirming those txs and assert the emitter produces anchors for them.
#[test]
fn unconfirmed_txs_anchored_on_confirmation() -> anyhow::Result<()> {
    let env = TestEnv::new()?;

    let addr_0 = env
        .rpc_client()
        .get_new_address(None, None)?
        .address()?
        .assume_checked();

    env.mine_blocks(101, None)?;

    let (mut chain, _) = LocalChain::from_genesis(env.genesis_hash()?);
    let mut indexed_tx_graph = IndexedTxGraph::<BlockId, _>::new({
        let mut index = SpkTxOutIndex::<usize>::default();
        index.insert_spk(0, addr_0.script_pubkey());
        index
    });

    let client = ClientExt::get_rpc_client(&env)?;
    let emitter = &mut Emitter::new(
        &client,
        chain.tip(),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        let _ = chain.apply_update(emission.checkpoint)?;
        let changeset = indexed_tx_graph.apply_block_relevant(&emission.block, height);
        assert!(changeset.is_empty());
    }

    // send 3 txs to a tracked address, these txs will be in the mempool
    let exp_txids = {
        let mut txids = BTreeSet::new();
        for _ in 0..3 {
            txids.insert(
                env.rpc_client()
                    .send_to_address(&addr_0, Amount::from_sat(10_000))?
                    .txid()?,
            );
        }
        txids
    };

    // expect that the next block should be none and we should get 3 txs from mempool
    {
        // next block should be `None`
        assert!(emitter.next_block()?.is_none());

        let mempool_txs = emitter.mempool()?;
        let changeset = indexed_tx_graph.batch_insert_unconfirmed(mempool_txs.update);
        assert_eq!(
            changeset
                .tx_graph
                .txs
                .iter()
                .map(|tx| tx.compute_txid())
                .collect::<BTreeSet<Txid>>(),
            exp_txids,
            "changeset should have the 3 mempool transactions",
        );
        assert!(changeset.tx_graph.anchors.is_empty());
    }

    // mine a block that confirms the 3 txs
    let exp_block_hash = env.mine_blocks(1, None)?[0];
    let exp_block_height = env
        .rpc_client()
        .get_block_verbose_one(exp_block_hash)?
        .height as u32;
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
        let changeset = indexed_tx_graph.apply_block_relevant(&emission.block, height);
        assert!(changeset.tx_graph.txs.is_empty());
        assert!(changeset.tx_graph.txouts.is_empty());
        assert_eq!(changeset.tx_graph.anchors, exp_anchors);
    }

    Ok(())
}

/// Ensure next block emitted after reorg is at reorg height.
///
/// After a reorg, if the last-emitted block height is equal or greater than the reorg height,
/// the next emission should be at the reorg height. This is guaranteed by the agreement-scanning
/// algorithm: the emitter walks back through its checkpoint list to find the deepest block still
/// in the best chain and resumes consecutive emission from there. Because `last_cp` is built from
/// the actual birthday hash (not just a height integer), the agreement point is always well-defined
/// regardless of how deep the reorg goes.
#[test]
fn ensure_block_emitted_after_reorg_is_at_reorg_height() -> anyhow::Result<()> {
    const EMITTER_START_HEIGHT: u64 = 100;
    const CHAIN_TIP_HEIGHT: usize = 110;

    let env = TestEnv::new()?;
    let client = ClientExt::get_rpc_client(&env)?;

    env.mine_blocks(CHAIN_TIP_HEIGHT, None)?;

    // Encode the birthday directly in last_cp rather than using a bare start_height integer.
    // This ensures agreement-scanning works correctly even when the birthday block is reorged out.
    let start_hash = env.get_block_hash(EMITTER_START_HEIGHT)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(EMITTER_START_HEIGHT as u32, start_hash),
        core::iter::empty::<bitcoin::Transaction>(),
    );

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
    recv_chain.apply_header(&block.header, block_height)?;
    let _ = recv_graph.apply_block(block, block_height);
    Ok(())
}

fn sync_from_emitter(
    recv_chain: &mut LocalChain,
    recv_graph: &mut IndexedTxGraph<BlockId, SpkTxOutIndex<()>>,
    emitter: &mut Emitter,
) -> anyhow::Result<()> {
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
    let outpoints = recv_graph.index.outpoints().clone();
    let balance = recv_chain
        .canonical_view(
            recv_graph.graph(),
            recv_chain.tip().block_id(),
            Default::default(),
        )
        .balance(outpoints, |_, _| true, 0);
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

    let client = ClientExt::get_rpc_client(&env)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // setup addresses
    let addr_to_mine = env
        .rpc_client()
        .get_new_address(None, None)?
        .address()?
        .assume_checked();
    let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, Network::Regtest)?;

    // setup receiver
    let (mut recv_chain, _) = LocalChain::from_genesis(env.genesis_hash()?);
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
            .get_transaction(txid)?
            .into_model()?
            .tx
            .output
            .into_iter()
            .enumerate()
            .filter(|(_, txo)| txo.script_pubkey == spk_to_track)
            .map(|(vout, _)| (txid, vout as u32))
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

/// Every call to mempool should return all currently-known unconfirmed transactions,
/// including ones returned on previous calls.
#[test]
fn mempool_update_is_complete_snapshot() -> anyhow::Result<()> {
    const BLOCKS_TO_MINE: usize = 101;
    const MEMPOOL_TX_COUNT: usize = 2;

    let env = TestEnv::new()?;

    let client = ClientExt::get_rpc_client(&env)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // mine blocks and sync up emitter
    let addr = env
        .rpc_client()
        .get_new_address(None, None)?
        .address()?
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

/// If a reorg invalidates the emitter's starting checkpoint, the emitter must find a lower
/// agreement point and resume consecutive emission from there.
///
/// 1. mine 101 blocks
/// 2. create emitter with last_cp at block 98 (one below the reorg point)
/// 3. emit blocks 99a, 100a
/// 4. reorg 3 blocks deep (replaces 99a, 100a, 101a with 99b, 100b, 101b)
/// 5. emit block 99b — agreement found at 98, next consecutive block is 99b
///
/// The block hash of 99b should be different than 99a, but their previous block hashes should
/// be the same (both build on block 98).
#[test]
fn reorg_past_start_checkpoint() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let client = ClientExt::get_rpc_client(&env)?;

    // mine 101 blocks first so block 98 exists for the checkpoint
    env.mine_blocks(100, None)?;

    assert_eq!(env.bitcoind.client.get_block_count()?.0, 101);

    // Encode last_cp at block 98 — the last block before the reorg zone.
    let cp_height: u64 = 98;
    let cp_hash = env.get_block_hash(cp_height)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(cp_height as u32, cp_hash),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // emit block 99a
    let event_99a = emitter.next_block()?.expect("block 99a header");
    assert_eq!(event_99a.block_height(), 99);
    let block_header_99a = event_99a.block.header;
    let block_hash_99a = block_header_99a.block_hash();
    let block_hash_98a = block_header_99a.prev_blockhash;

    // emit block 100a (advance the emitter past 99a so the reorg spans 3 blocks)
    let _block_100a = emitter.next_block()?.expect("block 100a header");

    // Reorg depth 3: invalidates 99a, 100a, 101a and mines new 99b, 100b, 101b.
    env.reorg(3)?;

    // emit block 99b: agreement found at block 98, which is unchanged, so next block is 99b
    let event_99b = emitter.next_block()?.expect("block 99b header");
    assert_eq!(event_99b.block_height(), 99);
    let block_header_99b = event_99b.block.header;
    let block_hash_99b = block_header_99b.block_hash();

    assert_ne!(block_hash_99a, block_hash_99b);
    assert_eq!(block_hash_98a, block_header_99a.prev_blockhash);
    assert_eq!(block_hash_98a, block_header_99b.prev_blockhash);

    Ok(())
}

/// Validates that when an unconfirmed transaction is double-spent (and thus evicted from the
/// mempool), the emitter reports it in `evicted`, and after inserting that eviction into the
/// graph it no longer appears in the set of canonical transactions.
///
/// 1. Broadcast a first tx (tx1) and confirm it arrives in unconfirmed set.
/// 2. Double-spend tx1 with tx1b and verify `mempool()` reports tx1 as evicted.
/// 3. Insert the eviction into the graph and assert tx1 is no longer canonical.
#[test]
fn double_spent_tx_evicted_and_removed_from_canonical_set() -> anyhow::Result<()> {
    use bdk_chain::miniscript;
    use bdk_chain::spk_txout::SpkTxOutIndex;
    use bitcoin::constants::genesis_block;
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::Network;
    let env = TestEnv::new()?;

    let desc_str = bdk_testenv::utils::DESCRIPTORS[0];
    let descriptor = miniscript::Descriptor::parse_descriptor(&Secp256k1::new(), desc_str)
        .unwrap()
        .0;
    let spk = descriptor.at_derivation_index(0)?.script_pubkey();

    let mut chain = LocalChain::from_genesis(genesis_block(Network::Regtest).block_hash()).0;
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
    let tx_1 = env.rpc_client().get_transaction(txid_1)?.into_model()?.tx;

    let client = ClientExt::get_rpc_client(&env)?;
    let mut emitter = Emitter::new(&client, chain.tip(), core::iter::once(tx_1));
    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        chain.apply_header(&emission.block.header, height)?;
    }

    let changeset = graph.batch_insert_unconfirmed(emitter.mempool()?.update);
    assert!(changeset
        .tx_graph
        .txs
        .iter()
        .any(|tx| tx.compute_txid() == txid_1));

    // Double spend tx1.

    // Get `prevout` from bitcoin core.
    let rpc_client = env.rpc_client();
    let tx1 = rpc_client.get_transaction(txid_1)?.into_model()?.tx;
    let txin = &tx1.input[0];
    let op = txin.previous_output;

    // Create `tx1b` using the previous output from tx1.
    let input = Input {
        txid: op.txid,
        vout: op.vout as u64,
        sequence: None,
    };

    let addr = rpc_client
        .get_new_address(None, None)?
        .address()?
        .assume_checked();

    let outputs = [Output::new(addr, Amount::from_btc(49.99)?)];
    let tx = rpc_client
        .create_raw_transaction(&[input], &outputs)?
        .into_model()?
        .0;
    let tx1b = rpc_client
        .sign_raw_transaction_with_wallet(&tx)?
        .into_model()?
        .tx;

    // Send the tx.
    let _txid_2 = rpc_client.send_raw_transaction(&tx1b)?;

    // Retrieve the expected unconfirmed txids and spks from the graph.
    let exp_spk_txids = chain
        .canonical_view(graph.graph(), chain_tip, Default::default())
        .list_expected_spk_txids(&graph.index, ..)
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

    let canonical_txids = chain
        .canonical_view(graph.graph(), chain_tip, Default::default())
        .txs()
        .map(|tx| tx.txid)
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
        .address()?
        .require_network(Network::Regtest)?;

    let client = ClientExt::get_rpc_client(&env)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::empty::<bitcoin::Transaction>(),
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

/// Verifies that encoding a birthday block directly in `last_cp` causes the emitter to skip all
/// blocks at or below the checkpoint height, starting emission from the next block.
#[test]
fn birthday_checkpoint_skips_earlier_blocks() -> anyhow::Result<()> {
    const BIRTHDAY_HEIGHT: u64 = 50;
    const CHAIN_TIP: usize = 101;

    let env = TestEnv::new()?;
    let client = ClientExt::get_rpc_client(&env)?;

    env.mine_blocks(CHAIN_TIP, None)?;

    let birthday_hash = env.get_block_hash(BIRTHDAY_HEIGHT)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(BIRTHDAY_HEIGHT as u32, birthday_hash),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    let mut emitted_heights = Vec::new();
    while let Some(event) = emitter.next_block()? {
        emitted_heights.push(event.block_height());
    }

    assert!(
        !emitted_heights.is_empty(),
        "should emit blocks above birthday"
    );
    assert_eq!(
        emitted_heights.first().copied(),
        Some(BIRTHDAY_HEIGHT as u32 + 1),
        "first emitted block must be immediately after birthday"
    );
    assert!(
        emitted_heights.iter().all(|&h| h > BIRTHDAY_HEIGHT as u32),
        "no block at or below birthday height should be emitted"
    );

    Ok(())
}

/// Verifies that all `(tx, ts)` pairs in a [`MempoolEvent`] carry the exact `sync_time` passed
/// to [`Emitter::mempool_at`], ensuring callers control the timestamp semantics.
#[test]
fn mempool_at_uses_provided_timestamp() -> anyhow::Result<()> {
    const SYNC_TIME: u64 = 42;

    let env = TestEnv::new()?;
    env.mine_blocks(101, None)?;

    let addr = env
        .rpc_client()
        .get_new_address(None, None)?
        .address()?
        .require_network(Network::Regtest)?;

    let client = ClientExt::get_rpc_client(&env)?;
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // Advance to tip so the emitter tracks the current chain position.
    while emitter.next_block()?.is_some() {}

    // Place a few transactions in the mempool.
    for _ in 0..3 {
        env.send(&addr, Amount::ONE_BTC)?;
    }

    let event = emitter.mempool_at(SYNC_TIME)?;

    assert!(
        !event.update.is_empty(),
        "should have received mempool transactions"
    );
    for (_, sync_time) in &event.update {
        assert_eq!(
            *sync_time, SYNC_TIME,
            "all update timestamps must equal sync_time"
        );
    }

    Ok(())
}

/// Verifies that when a reorg's agreement point falls below `start_height`, the emitter resets
/// `start_height` to the agreement height so that no reorged heights are skipped.
///
/// Concretely, with a 6-block reorg the emitter must re-emit all 6 reorged heights in order
/// without skipping any, even though `start_height` was set to the pre-reorg tip.
#[test]
fn start_height_reset_on_reorg_prevents_height_gaps() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let (mut local_chain, _) = LocalChain::from_genesis(env.genesis_hash()?);
    let client = ClientExt::get_rpc_client(&env)?;

    const REORG_DEPTH: u32 = 6;

    env.mine_blocks(110, None)?;

    let mut emitter = Emitter::new(
        &client,
        local_chain.tip(),
        core::iter::empty::<Arc<Transaction>>(),
    );
    while let Some(emission) = emitter.next_block()? {
        let _ = local_chain.apply_update(emission.checkpoint)?;
    }

    let pre_reorg_tip = local_chain.tip();
    let tip_height = pre_reorg_tip.height();

    env.reorg(REORG_DEPTH as usize)?;

    // New emitter with start_height = tip height. The emitter must detect the reorg, walk back
    // to the agreement point, and reset start_height so no invalidated heights are skipped.
    let mut emitter = Emitter::new(
        &client,
        local_chain.tip(),
        core::iter::empty::<Arc<Transaction>>(),
    )
    .start_height(tip_height);

    let mut emitted_heights = Vec::new();
    while let Some(emission) = emitter.next_block()? {
        emitted_heights.push(emission.block_height());
        let _ = local_chain
            .apply_update(emission.checkpoint)
            .expect("emission checkpoint must connect with local chain");
    }

    // All reorged heights must be re-emitted consecutively — no gaps.
    let reorg_start = tip_height - REORG_DEPTH + 1;
    let exp_heights: Vec<u32> = (reorg_start..=tip_height).collect();
    assert_eq!(
        emitted_heights, exp_heights,
        "emitter must re-emit all reorged heights without skipping; \
         got {:?}, expected {:?}",
        emitted_heights, exp_heights,
    );

    assert_eq!(local_chain.tip().height(), tip_height);
    assert_ne!(local_chain.tip().hash(), pre_reorg_tip.hash());

    Ok(())
}

/// Evictions are withheld while the emitter is behind the node's best-block tip and are only
/// surfaced once [`Emitter::next_block`] has drained the chain to tip. This applies both to
/// live evictions and to transactions seeded via `expected_mempool_txs` at construction time
/// (the wallet-restart scenario).
///
/// **Phase 1 — live eviction while catching up:**
/// 1. Mine 110 blocks; emitter starts at genesis — behind tip.
/// 2. Broadcast tx1; `mempool()` shows tx1 in `update`, `evicted` is empty (behind tip).
/// 3. Double-spend tx1 (tx1b) to evict it from the mempool.
/// 4. Call `mempool()` again — still behind tip, `evicted` must still be empty.
/// 5. Drain all blocks to tip; `mempool()` — tx1 must appear in `evicted`.
///
/// **Phase 2 — wallet-restart: seeded tx already absent from mempool:**
/// 6. Create a fresh emitter from genesis, seeding tx1 (already evicted) as a known-unconfirmed tx.
/// 7. Drain all blocks to tip; `mempool()` — tx1 must again appear in `evicted`.
#[test]
fn evictions_withheld_until_at_tip() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let rpc_client = env.rpc_client();
    let client = ClientExt::get_rpc_client(&env)?;

    env.mine_blocks(110, None)?;

    let spk = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let recipient = Address::from_script(&spk, Network::Regtest)?;
    let txid_1 = env.send(&recipient, Amount::from_sat(10_000))?;
    // Fetch the full tx now so we can seed it in phase 2.
    let tx_1 = rpc_client.get_transaction(txid_1)?.into_model()?.tx;

    // --- Phase 1: live eviction while catching up ---

    // Emitter starts at genesis — deliberately behind the node's tip.
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // Behind tip: tx1 appears in update, but no evictions are reported yet.
    let event = emitter.mempool()?;
    assert!(
        event
            .update
            .iter()
            .any(|(tx, _)| tx.compute_txid() == txid_1),
        "phase 1: tx1 should appear in mempool update",
    );
    assert!(
        event.evicted.is_empty(),
        "phase 1: evicted must be empty while emitter is behind tip",
    );

    // Double-spend tx1 to evict it from the mempool.
    let outpoint = tx_1.input[0].previous_output;
    let new_addr = rpc_client
        .get_new_address(None, None)?
        .address()?
        .assume_checked();
    let input = Input {
        txid: outpoint.txid,
        vout: outpoint.vout as u64,
        sequence: None,
    };
    let outputs = [Output::new(new_addr, Amount::from_btc(49.99)?)];
    let raw = rpc_client
        .create_raw_transaction(&[input], &outputs)?
        .into_model()?
        .0;
    let tx1b = rpc_client
        .sign_raw_transaction_with_wallet(&raw)?
        .into_model()?
        .tx;
    rpc_client.send_raw_transaction(&tx1b)?;

    // Still behind tip: evicted must remain empty even though tx1 is gone from the mempool.
    let event = emitter.mempool()?;
    assert!(
        event.evicted.is_empty(),
        "phase 1: evicted must remain empty while emitter is still catching up",
    );

    // Drain all blocks to tip.
    while emitter.next_block()?.is_some() {}

    // Now at tip: tx1 was evicted and must be reported.
    let event = emitter.mempool()?;
    assert!(
        event.evicted.iter().any(|(txid, _)| txid == &txid_1),
        "phase 1: tx1 must appear in evicted once emitter is at tip",
    );

    // --- Phase 2: wallet restart — seeded tx already absent from mempool ---

    // tx1 is still evicted. Simulate a wallet restart by creating a fresh emitter from
    // genesis and seeding tx1 as a known-unconfirmed transaction.
    let mut emitter2 = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::once(tx_1),
    );

    // Drain all blocks to tip. tx1 is not confirmed in any of them (it was evicted, not mined).
    while emitter2.next_block()?.is_some() {}

    // At tip: the snapshot has tx1 (from seeding) but the node's mempool does not. Must be evicted.
    let event = emitter2.mempool()?;
    assert!(
        event.evicted.iter().any(|(txid, _)| txid == &txid_1),
        "phase 2: seeded-but-absent tx1 must be reported as evicted once at tip",
    );

    Ok(())
}

/// Setting `start_height` to a height that does not yet exist on the remote node results in
/// [`EmitterError::Rpc`] on the first [`Emitter::next_block`] call that tries to fetch that block.
///
/// This is the documented contract: callers should not set `start_height` beyond the node's
/// current tip. The error is recoverable — the caller can reinitialise with a valid height.
#[test]
fn start_height_beyond_tip_returns_rpc_error() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let client = ClientExt::get_rpc_client(&env)?;

    // Mine enough to be able to coinbase-spend, but well short of 999_999.
    env.mine_blocks(10, None)?;

    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, env.genesis_hash()?),
        core::iter::empty::<bitcoin::Transaction>(),
    )
    .start_height(999_999);

    // The emitter finds agreement at genesis, then tries get_block_hash(999_999) which the
    // node rejects because that height does not exist.
    let err = emitter
        .next_block()
        .expect_err("must fail with Rpc error for out-of-range start_height");
    assert!(
        matches!(err, EmitterError::Rpc(_)),
        "expected EmitterError::Rpc, got {err:?}",
    );

    Ok(())
}

/// An emitter whose `last_cp` has a genesis hash that does not exist on the connected node
/// (e.g. a mainnet genesis hash used against a regtest node) exhausts all agreement candidates
/// and returns [`EmitterError::AgreementNotFound`].
///
/// This is the documented "catastrophic mismatch" safety net: the caller should reinitialise
/// the emitter against the correct node.
#[test]
fn wrong_genesis_returns_agreement_not_found() -> anyhow::Result<()> {
    use bitcoin::constants::genesis_block;

    let env = TestEnv::new()?;
    let client = ClientExt::get_rpc_client(&env)?;

    env.mine_blocks(10, None)?;

    // Use the mainnet genesis hash — it will never exist on the regtest node.
    let mainnet_genesis = genesis_block(bitcoin::params::Params::MAINNET).block_hash();
    let mut emitter = Emitter::new(
        &client,
        CheckPoint::new(0, mainnet_genesis),
        core::iter::empty::<bitcoin::Transaction>(),
    );

    // The agreement scanner finds no matching block and, after MAX_AGREEMENT_FAILURES retries
    // within this single call, surfaces the error.
    let err = emitter
        .next_block()
        .expect_err("must fail with AgreementNotFound for wrong genesis hash");
    assert!(
        matches!(err, EmitterError::AgreementNotFound),
        "expected EmitterError::AgreementNotFound, got {err:?}",
    );

    Ok(())
}
