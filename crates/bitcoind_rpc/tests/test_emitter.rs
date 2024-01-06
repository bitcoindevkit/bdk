use core::panic;
use std::collections::{BTreeMap, BTreeSet};

use bdk_bitcoind_rpc::Emitter;
use bdk_chain::{
    bitcoin::{Address, Amount, BlockHash, Txid},
    keychain::Balance,
    local_chain::{self, CheckPoint, LocalChain},
    Append, BlockId, IndexedTxGraph, SpkTxOutIndex,
};
use bitcoin::{
    address::NetworkChecked, block::Header, hash_types::TxMerkleNode, hashes::Hash,
    secp256k1::rand::random, Block, CompactTarget, OutPoint, ScriptBuf, ScriptHash, Transaction,
    TxIn, TxOut, WScriptHash,
};
use bitcoincore_rpc::{
    bitcoincore_rpc_json::{GetBlockTemplateModes, GetBlockTemplateRules},
    RpcApi,
};

struct TestEnv {
    #[allow(dead_code)]
    daemon: bitcoind::BitcoinD,
    client: bitcoincore_rpc::Client,
}

impl TestEnv {
    fn new() -> anyhow::Result<Self> {
        let daemon = match std::env::var_os("BITCOIND_EXEC") {
            Some(bitcoind_path) => bitcoind::BitcoinD::new(bitcoind_path),
            None => panic!("Cannot find bitcoind daemon, set BITCOIND_EXEC environment variable with the path to bitcoind"),
        }?;
        let client = bitcoincore_rpc::Client::new(
            &daemon.rpc_url(),
            bitcoincore_rpc::Auth::CookieFile(daemon.params.cookie_file.clone()),
        )?;
        Ok(Self { daemon, client })
    }

    fn mine_blocks(
        &self,
        count: usize,
        address: Option<Address>,
    ) -> anyhow::Result<Vec<BlockHash>> {
        let coinbase_address = match address {
            Some(address) => address,
            None => self.client.get_new_address(None, None)?.assume_checked(),
        };
        let block_hashes = self
            .client
            .generate_to_address(count as _, &coinbase_address)?;
        Ok(block_hashes)
    }

    fn mine_empty_block(&self) -> anyhow::Result<(usize, BlockHash)> {
        let bt = self.client.get_block_template(
            GetBlockTemplateModes::Template,
            &[GetBlockTemplateRules::SegWit],
            &[],
        )?;

        let txdata = vec![Transaction {
            version: 1,
            lock_time: bitcoin::absolute::LockTime::from_height(0)?,
            input: vec![TxIn {
                previous_output: bitcoin::OutPoint::default(),
                script_sig: ScriptBuf::builder()
                    .push_int(bt.height as _)
                    // randomn number so that re-mining creates unique block
                    .push_int(random())
                    .into_script(),
                sequence: bitcoin::Sequence::default(),
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: 0,
                script_pubkey: ScriptBuf::new_p2sh(&ScriptHash::all_zeros()),
            }],
        }];

        let bits: [u8; 4] = bt
            .bits
            .clone()
            .try_into()
            .expect("rpc provided us with invalid bits");

        let mut block = Block {
            header: Header {
                version: bitcoin::block::Version::default(),
                prev_blockhash: bt.previous_block_hash,
                merkle_root: TxMerkleNode::all_zeros(),
                time: Ord::max(bt.min_time, std::time::UNIX_EPOCH.elapsed()?.as_secs()) as u32,
                bits: CompactTarget::from_consensus(u32::from_be_bytes(bits)),
                nonce: 0,
            },
            txdata,
        };

        block.header.merkle_root = block.compute_merkle_root().expect("must compute");

        for nonce in 0..=u32::MAX {
            block.header.nonce = nonce;
            if block.header.target().is_met_by(block.block_hash()) {
                break;
            }
        }

        self.client.submit_block(&block)?;
        Ok((bt.height as usize, block.block_hash()))
    }

    fn invalidate_blocks(&self, count: usize) -> anyhow::Result<()> {
        let mut hash = self.client.get_best_block_hash()?;
        for _ in 0..count {
            let prev_hash = self.client.get_block_info(&hash)?.previousblockhash;
            self.client.invalidate_block(&hash)?;
            match prev_hash {
                Some(prev_hash) => hash = prev_hash,
                None => break,
            }
        }
        Ok(())
    }

    fn reorg(&self, count: usize) -> anyhow::Result<Vec<BlockHash>> {
        let start_height = self.client.get_block_count()?;
        self.invalidate_blocks(count)?;

        let res = self.mine_blocks(count, None);
        assert_eq!(
            self.client.get_block_count()?,
            start_height,
            "reorg should not result in height change"
        );
        res
    }

    fn reorg_empty_blocks(&self, count: usize) -> anyhow::Result<Vec<(usize, BlockHash)>> {
        let start_height = self.client.get_block_count()?;
        self.invalidate_blocks(count)?;

        let res = (0..count)
            .map(|_| self.mine_empty_block())
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            self.client.get_block_count()?,
            start_height,
            "reorg should not result in height change"
        );
        Ok(res)
    }

    fn send(&self, address: &Address<NetworkChecked>, amount: Amount) -> anyhow::Result<Txid> {
        let txid = self
            .client
            .send_to_address(address, amount, None, None, None, None, None, None)?;
        Ok(txid)
    }
}

/// Ensure that blocks are emitted in order even after reorg.
///
/// 1. Mine 101 blocks.
/// 2. Emit blocks from [`Emitter`] and update the [`LocalChain`].
/// 3. Reorg highest 6 blocks.
/// 4. Emit blocks from [`Emitter`] and re-update the [`LocalChain`].
#[test]
pub fn test_sync_local_chain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let (mut local_chain, _) = LocalChain::from_genesis_hash(env.client.get_block_hash(0)?);
    let mut emitter = Emitter::new(&env.client, local_chain.tip(), 0);

    // mine some blocks and returned the actual block hashes
    let exp_hashes = {
        let mut hashes = vec![env.client.get_block_hash(0)?]; // include genesis block
        hashes.extend(env.mine_blocks(101, None)?);
        hashes
    };

    // see if the emitter outputs the right blocks
    println!("first sync:");
    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        let hash = emission.block_hash();
        assert_eq!(
            emission.block_hash(),
            exp_hashes[height as usize],
            "emitted block hash is unexpected"
        );

        assert_eq!(
            local_chain.apply_update(local_chain::Update {
                tip: emission.checkpoint,
                introduce_older_blocks: false,
            })?,
            BTreeMap::from([(height, Some(hash))]),
            "chain update changeset is unexpected",
        );
    }

    assert_eq!(
        local_chain.blocks(),
        &exp_hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| (i as u32, *hash))
            .collect(),
        "final local_chain state is unexpected",
    );

    // perform reorg
    let reorged_blocks = env.reorg(6)?;
    let exp_hashes = exp_hashes
        .iter()
        .take(exp_hashes.len() - reorged_blocks.len())
        .chain(&reorged_blocks)
        .cloned()
        .collect::<Vec<_>>();

    // see if the emitter outputs the right blocks
    println!("after reorg:");
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
            local_chain.apply_update(local_chain::Update {
                tip: emission.checkpoint,
                introduce_older_blocks: false,
            })?,
            if exp_height == exp_hashes.len() - reorged_blocks.len() {
                core::iter::once((height, Some(hash)))
                    .chain((height + 1..exp_hashes.len() as u32).map(|h| (h, None)))
                    .collect::<bdk_chain::local_chain::ChangeSet>()
            } else {
                BTreeMap::from([(height, Some(hash))])
            },
            "chain update changeset is unexpected",
        );

        exp_height += 1;
    }

    assert_eq!(
        local_chain.blocks(),
        &exp_hashes
            .iter()
            .enumerate()
            .map(|(i, hash)| (i as u32, *hash))
            .collect(),
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

    println!("getting new addresses!");
    let addr_0 = env.client.get_new_address(None, None)?.assume_checked();
    let addr_1 = env.client.get_new_address(None, None)?.assume_checked();
    let addr_2 = env.client.get_new_address(None, None)?.assume_checked();
    println!("got new addresses!");

    println!("mining block!");
    env.mine_blocks(101, None)?;
    println!("mined blocks!");

    let (mut chain, _) = LocalChain::from_genesis_hash(env.client.get_block_hash(0)?);
    let mut indexed_tx_graph = IndexedTxGraph::<BlockId, _>::new({
        let mut index = SpkTxOutIndex::<usize>::default();
        index.insert_spk(0, addr_0.script_pubkey());
        index.insert_spk(1, addr_1.script_pubkey());
        index.insert_spk(2, addr_2.script_pubkey());
        index
    });

    let emitter = &mut Emitter::new(&env.client, chain.tip(), 0);

    while let Some(emission) = emitter.next_block()? {
        let height = emission.block_height();
        let _ = chain.apply_update(local_chain::Update {
            tip: emission.checkpoint,
            introduce_older_blocks: false,
        })?;
        let indexed_additions = indexed_tx_graph.apply_block_relevant(&emission.block, height);
        assert!(indexed_additions.is_empty());
    }

    // send 3 txs to a tracked address, these txs will be in the mempool
    let exp_txids = {
        let mut txids = BTreeSet::new();
        for _ in 0..3 {
            txids.insert(env.client.send_to_address(
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
        let indexed_additions = indexed_tx_graph.batch_insert_unconfirmed(mempool_txs);
        assert_eq!(
            indexed_additions
                .graph
                .txs
                .iter()
                .map(|tx| tx.txid())
                .collect::<BTreeSet<Txid>>(),
            exp_txids,
            "changeset should have the 3 mempool transactions",
        );
        assert!(indexed_additions.graph.anchors.is_empty());
    }

    // mine a block that confirms the 3 txs
    let exp_block_hash = env.mine_blocks(1, None)?[0];
    let exp_block_height = env.client.get_block_info(&exp_block_hash)?.height as u32;
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
        let _ = chain.apply_update(local_chain::Update {
            tip: emission.checkpoint,
            introduce_older_blocks: false,
        })?;
        let indexed_additions = indexed_tx_graph.apply_block_relevant(&emission.block, height);
        assert!(indexed_additions.graph.txs.is_empty());
        assert!(indexed_additions.graph.txouts.is_empty());
        assert_eq!(indexed_additions.graph.anchors, exp_anchors);
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
        &env.client,
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.client.get_block_hash(0)?,
        }),
        EMITTER_START_HEIGHT as _,
    );

    env.mine_blocks(CHAIN_TIP_HEIGHT, None)?;
    while emitter.next_header()?.is_some() {}

    for reorg_count in 1..=10 {
        let replaced_blocks = env.reorg_empty_blocks(reorg_count)?;
        let next_emission = emitter.next_header()?.expect("must emit block after reorg");
        assert_eq!(
            (
                next_emission.block_height() as usize,
                next_emission.block_hash()
            ),
            replaced_blocks[0],
            "block emitted after reorg should be at the reorg height"
        );
        while emitter.next_header()?.is_some() {}
    }

    Ok(())
}

fn process_block(
    recv_chain: &mut LocalChain,
    recv_graph: &mut IndexedTxGraph<BlockId, SpkTxOutIndex<()>>,
    block: Block,
    block_height: u32,
) -> anyhow::Result<()> {
    recv_chain
        .apply_update(CheckPoint::from_header(&block.header, block_height).into_update(false))?;
    let _ = recv_graph.apply_block(block, block_height);
    Ok(())
}

fn sync_from_emitter<C>(
    recv_chain: &mut LocalChain,
    recv_graph: &mut IndexedTxGraph<BlockId, SpkTxOutIndex<()>>,
    emitter: &mut Emitter<C>,
) -> anyhow::Result<()>
where
    C: bitcoincore_rpc::RpcApi,
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
    let balance = recv_graph
        .graph()
        .balance(recv_chain, chain_tip, outpoints, |_, _| true);
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
        &env.client,
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.client.get_block_hash(0)?,
        }),
        0,
    );

    // setup addresses
    let addr_to_mine = env.client.get_new_address(None, None)?.assume_checked();
    let spk_to_track = ScriptBuf::new_v0_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, bitcoin::Network::Regtest)?;

    // setup receiver
    let (mut recv_chain, _) = LocalChain::from_genesis_hash(env.client.get_block_hash(0)?);
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
            .client
            .get_transaction(&txid, None)?
            .transaction()?
            .output
            .into_iter()
            .enumerate()
            .filter(|(_, txo)| txo.script_pubkey == spk_to_track)
            .map(|(vout, _)| OutPoint::new(txid, vout as _))
            .collect::<Vec<_>>();
        env.client.lock_unspent(&outpoints_to_lock)?;

        let _ = env.mine_blocks(1, None)?;
    }

    // get emitter up to tip
    sync_from_emitter(&mut recv_chain, &mut recv_graph, &mut emitter)?;

    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            confirmed: SEND_AMOUNT.to_sat() * ADDITIONAL_COUNT as u64,
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
                confirmed: SEND_AMOUNT.to_sat() * (ADDITIONAL_COUNT - reorg_count) as u64,
                trusted_pending: SEND_AMOUNT.to_sat() * reorg_count as u64,
                ..Balance::default()
            },
            "reorg_count: {}",
            reorg_count,
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
        &env.client,
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.client.get_block_hash(0)?,
        }),
        0,
    );

    // mine blocks and sync up emitter
    let addr = env.client.get_new_address(None, None)?.assume_checked();
    env.mine_blocks(BLOCKS_TO_MINE, Some(addr.clone()))?;
    while emitter.next_header()?.is_some() {}

    // have some random txs in mempool
    let exp_txids = (0..MEMPOOL_TX_COUNT)
        .map(|_| env.send(&addr, Amount::from_sat(2100)))
        .collect::<Result<BTreeSet<Txid>, _>>()?;

    // the first emission should include all transactions
    let emitted_txids = emitter
        .mempool()?
        .into_iter()
        .map(|(tx, _)| tx.txid())
        .collect::<BTreeSet<Txid>>();
    assert_eq!(
        emitted_txids, exp_txids,
        "all mempool txs should be emitted"
    );

    // second emission should be empty
    assert!(
        emitter.mempool()?.is_empty(),
        "second emission should be empty"
    );

    // mine empty blocks + sync up our emitter -> we should still not re-emit
    for _ in 0..BLOCKS_TO_MINE {
        env.mine_empty_block()?;
    }
    while emitter.next_header()?.is_some() {}
    assert!(
        emitter.mempool()?.is_empty(),
        "third emission, after chain tip is extended, should also be empty"
    );

    Ok(())
}

/// Ensure mempool tx is still re-emitted if [`Emitter`] has not reached the tx's introduction
/// height.
///
/// We introduce a mempool tx after each block, where blocks are empty (does not confirm previous
/// mempool txs). Then we emit blocks from [`Emitter`] (intertwining `mempool` calls). We check
/// that `mempool` should always re-emit txs that have introduced at a height greater than the last
/// emitted block height.
#[test]
fn mempool_re_emits_if_tx_introduction_height_not_reached() -> anyhow::Result<()> {
    const PREMINE_COUNT: usize = 101;
    const MEMPOOL_TX_COUNT: usize = 21;

    let env = TestEnv::new()?;
    let mut emitter = Emitter::new(
        &env.client,
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.client.get_block_hash(0)?,
        }),
        0,
    );

    // mine blocks to get initial balance, sync emitter up to tip
    let addr = env.client.get_new_address(None, None)?.assume_checked();
    env.mine_blocks(PREMINE_COUNT, Some(addr.clone()))?;
    while emitter.next_header()?.is_some() {}

    // mine blocks to introduce txs to mempool at different heights
    let tx_introductions = (0..MEMPOOL_TX_COUNT)
        .map(|_| -> anyhow::Result<_> {
            let (height, _) = env.mine_empty_block()?;
            let txid = env.send(&addr, Amount::from_sat(2100))?;
            Ok((height, txid))
        })
        .collect::<anyhow::Result<BTreeSet<_>>>()?;

    assert_eq!(
        emitter
            .mempool()?
            .into_iter()
            .map(|(tx, _)| tx.txid())
            .collect::<BTreeSet<_>>(),
        tx_introductions.iter().map(|&(_, txid)| txid).collect(),
        "first mempool emission should include all txs",
    );
    assert_eq!(
        emitter
            .mempool()?
            .into_iter()
            .map(|(tx, _)| tx.txid())
            .collect::<BTreeSet<_>>(),
        tx_introductions.iter().map(|&(_, txid)| txid).collect(),
        "second mempool emission should still include all txs",
    );

    // At this point, the emitter has seen all mempool transactions. It should only re-emit those
    // that have introduction heights less than the emitter's last-emitted block tip.
    while let Some(emission) = emitter.next_header()? {
        let height = emission.block_height();
        // We call `mempool()` twice.
        // The second call (at height `h`) should skip the tx introduced at height `h`.
        for try_index in 0..2 {
            let exp_txids = tx_introductions
                .range((height as usize + try_index, Txid::all_zeros())..)
                .map(|&(_, txid)| txid)
                .collect::<BTreeSet<_>>();
            let emitted_txids = emitter
                .mempool()?
                .into_iter()
                .map(|(tx, _)| tx.txid())
                .collect::<BTreeSet<_>>();
            assert_eq!(
                emitted_txids, exp_txids,
                "\n emission {} (try {}) must only contain txs introduced at that height or lower: \n\t missing: {:?} \n\t extra: {:?}",
                height,
                try_index,
                exp_txids
                    .difference(&emitted_txids)
                    .map(|txid| (txid, tx_introductions.iter().find_map(|(h, id)| if id == txid { Some(h) } else { None }).unwrap()))
                    .collect::<Vec<_>>(),
                emitted_txids
                    .difference(&exp_txids)
                    .map(|txid| (txid, tx_introductions.iter().find_map(|(h, id)| if id == txid { Some(h) } else { None }).unwrap()))
                    .collect::<Vec<_>>(),
            );
        }
    }

    Ok(())
}

/// Ensure we force re-emit all mempool txs after reorg.
#[test]
fn mempool_during_reorg() -> anyhow::Result<()> {
    const TIP_DIFF: usize = 10;
    const PREMINE_COUNT: usize = 101;

    let env = TestEnv::new()?;
    let mut emitter = Emitter::new(
        &env.client,
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.client.get_block_hash(0)?,
        }),
        0,
    );

    // mine blocks to get initial balance
    let addr = env.client.get_new_address(None, None)?.assume_checked();
    env.mine_blocks(PREMINE_COUNT, Some(addr.clone()))?;

    // introduce mempool tx at each block extension
    for _ in 0..TIP_DIFF {
        env.mine_empty_block()?;
        env.send(&addr, Amount::from_sat(2100))?;
    }

    // sync emitter to tip, first mempool emission should include all txs (as we haven't emitted
    // from the mempool yet)
    while emitter.next_header()?.is_some() {}
    assert_eq!(
        emitter
            .mempool()?
            .into_iter()
            .map(|(tx, _)| tx.txid())
            .collect::<BTreeSet<_>>(),
        env.client
            .get_raw_mempool()?
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "first mempool emission should include all txs",
    );

    // perform reorgs at different heights, these reorgs will not confirm transactions in the
    // mempool
    for reorg_count in 1..TIP_DIFF {
        println!("REORG COUNT: {}", reorg_count);
        env.reorg_empty_blocks(reorg_count)?;

        // This is a map of mempool txids to tip height where the tx was introduced to the mempool
        // we recalculate this at every loop as reorgs may evict transactions from mempool. We use
        // the introduction height to determine whether we expect a tx to appear in a mempool
        // emission.
        // TODO: How can have have reorg logic in `TestEnv` NOT blacklast old blocks first?
        let tx_introductions = dbg!(env
            .client
            .get_raw_mempool_verbose()?
            .into_iter()
            .map(|(txid, entry)| (txid, entry.height as usize))
            .collect::<BTreeMap<_, _>>());

        // `next_header` emits the replacement block of the reorg
        if let Some(emission) = emitter.next_header()? {
            let height = emission.block_height();
            println!("\t- replacement height: {}", height);

            // the mempool emission (that follows the first block emission after reorg) should only
            // include mempool txs introduced at reorg height or greater
            let mempool = emitter
                .mempool()?
                .into_iter()
                .map(|(tx, _)| tx.txid())
                .collect::<BTreeSet<_>>();
            let exp_mempool = tx_introductions
                .iter()
                .filter(|(_, &intro_h)| intro_h >= (height as usize))
                .map(|(&txid, _)| txid)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                mempool, exp_mempool,
                "the first mempool emission after reorg should only include mempool txs introduced at reorg height or greater"
            );

            let mempool = emitter
                .mempool()?
                .into_iter()
                .map(|(tx, _)| tx.txid())
                .collect::<BTreeSet<_>>();
            let exp_mempool = tx_introductions
                .iter()
                .filter(|&(_, &intro_height)| intro_height > (height as usize))
                .map(|(&txid, _)| txid)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                mempool, exp_mempool,
                "following mempool emissions after reorg should exclude mempool introduction heights <= last emitted block height: \n\t missing: {:?} \n\t extra: {:?}",
                exp_mempool
                    .difference(&mempool)
                    .map(|txid| (txid, tx_introductions.get(txid).unwrap()))
                    .collect::<Vec<_>>(),
                mempool
                    .difference(&exp_mempool)
                    .map(|txid| (txid, tx_introductions.get(txid).unwrap()))
                    .collect::<Vec<_>>(),
            );
        }

        // sync emitter to tip
        while emitter.next_header()?.is_some() {}
    }

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
        &env.client,
        CheckPoint::new(BlockId {
            height: 0,
            hash: env.client.get_block_hash(0)?,
        }),
        (PREMINE_COUNT - 2) as u32,
    );

    // mine 101 blocks
    env.mine_blocks(PREMINE_COUNT, None)?;

    // emit block 99a
    let block_header_99a = emitter.next_header()?.expect("block 99a header").block;
    let block_hash_99a = block_header_99a.block_hash();
    let block_hash_98a = block_header_99a.prev_blockhash;

    // emit block 100a
    let block_header_100a = emitter.next_header()?.expect("block 100a header").block;
    let block_hash_100a = block_header_100a.block_hash();

    // get hash for block 101a
    let block_hash_101a = env.client.get_block_hash(101)?;

    // invalidate blocks 99a, 100a, 101a
    env.client.invalidate_block(&block_hash_99a)?;
    env.client.invalidate_block(&block_hash_100a)?;
    env.client.invalidate_block(&block_hash_101a)?;

    // mine new blocks 99b, 100b, 101b
    env.mine_blocks(3, None)?;

    // emit block header 99b
    let block_header_99b = emitter.next_header()?.expect("block 99b header").block;
    let block_hash_99b = block_header_99b.block_hash();
    let block_hash_98b = block_header_99b.prev_blockhash;

    assert_ne!(block_hash_99a, block_hash_99b);
    assert_eq!(block_hash_98a, block_hash_98b);

    Ok(())
}
