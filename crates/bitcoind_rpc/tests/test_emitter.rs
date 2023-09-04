use std::collections::{BTreeMap, BTreeSet};

use bdk_bitcoind_rpc::{EmittedBlock, Emitter, MempoolTx};
use bdk_chain::{
    bitcoin::{Address, Amount, BlockHash, Txid},
    indexed_tx_graph::TxItem,
    local_chain::{self, CheckPoint, LocalChain},
    Append, BlockId, IndexedTxGraph, SpkTxOutIndex,
};
use bitcoin::hashes::Hash;
use bitcoincore_rpc::RpcApi;

struct TestEnv {
    #[allow(dead_code)]
    daemon: bitcoind::BitcoinD,
    client: bitcoincore_rpc::Client,
}

impl TestEnv {
    fn new() -> anyhow::Result<Self> {
        let daemon = match std::env::var_os("TEST_BITCOIND") {
            Some(bitcoind_path) => bitcoind::BitcoinD::new(bitcoind_path),
            None => bitcoind::BitcoinD::from_downloaded(),
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

    fn reorg(&self, count: usize) -> anyhow::Result<Vec<BlockHash>> {
        let start_height = self.client.get_block_count()?;

        let mut hash = self.client.get_best_block_hash()?;
        for _ in 0..count {
            let prev_hash = self.client.get_block_info(&hash)?.previousblockhash;
            self.client.invalidate_block(&hash)?;
            match prev_hash {
                Some(prev_hash) => hash = prev_hash,
                None => break,
            }
        }

        let res = self.mine_blocks(count, None);
        assert_eq!(
            self.client.get_block_count()?,
            start_height,
            "reorg should not result in height change"
        );
        res
    }
}

fn block_to_chain_update(block: &bitcoin::Block, height: u32) -> local_chain::Update {
    let this_id = BlockId {
        height,
        hash: block.block_hash(),
    };
    let tip = if block.header.prev_blockhash == BlockHash::all_zeros() {
        CheckPoint::new(this_id)
    } else {
        CheckPoint::new(BlockId {
            height: height - 1,
            hash: block.header.prev_blockhash,
        })
        .extend(core::iter::once(this_id))
        .expect("must construct checkpoint")
    };

    local_chain::Update {
        tip,
        introduce_older_blocks: false,
    }
}

fn block_to_tx_graph_update(
    block: &bitcoin::Block,
    height: u32,
) -> impl Iterator<Item = TxItem<'_, Option<BlockId>>> {
    let anchor = BlockId {
        hash: block.block_hash(),
        height,
    };
    block.txdata.iter().map(move |tx| (tx, Some(anchor), None))
}

fn mempool_to_tx_graph_update(
    mempool_txs: &[MempoolTx],
) -> impl Iterator<Item = TxItem<'_, Option<BlockId>>> {
    mempool_txs
        .iter()
        .map(|mempool_tx| (&mempool_tx.tx, None, Some(mempool_tx.time)))
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
    let mut local_chain = LocalChain::default();
    let mut emitter = Emitter::new(&env.client, 0);

    // mine some blocks and returned the actual block hashes
    let exp_hashes = {
        let mut hashes = vec![env.client.get_block_hash(0)?]; // include genesis block
        hashes.extend(env.mine_blocks(101, None)?);
        hashes
    };

    // see if the emitter outputs the right blocks
    println!("first sync:");
    loop {
        let (height, block) = match emitter.next_block()? {
            Some(b) => {
                println!("\t[{:3}] {}", b.height, b.block.block_hash());
                (b.height, b.block)
            }
            None => break,
        };
        assert_eq!(
            block.block_hash(),
            exp_hashes[height as usize],
            "emitted block hash is unexpected"
        );

        let chain_update = block_to_chain_update(&block, height);
        assert_eq!(
            local_chain.apply_update(chain_update)?,
            BTreeMap::from([(height, Some(block.block_hash()))]),
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
    loop {
        let (height, block) = match emitter.next_block()? {
            Some(b) => {
                println!("\t[{:3}] {}", b.height, b.block.block_hash());
                (b.height, b.block)
            }
            None => break,
        };
        assert_eq!(
            height, exp_height as u32,
            "emitted block has unexpected height"
        );

        assert_eq!(
            block.block_hash(),
            exp_hashes[height as usize],
            "emitted block is unexpected"
        );

        let chain_update = block_to_chain_update(&block, height);
        assert_eq!(
            local_chain.apply_update(chain_update)?,
            if exp_height == exp_hashes.len() - reorged_blocks.len() {
                core::iter::once((height, Some(block.block_hash())))
                    .chain((height + 1..exp_hashes.len() as u32).map(|h| (h, None)))
                    .collect::<bdk_chain::local_chain::ChangeSet>()
            } else {
                BTreeMap::from([(height, Some(block.block_hash()))])
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

    let mut chain = LocalChain::default();
    let mut indexed_tx_graph = IndexedTxGraph::<BlockId, _>::new({
        let mut index = SpkTxOutIndex::<usize>::default();
        index.insert_spk(0, addr_0.script_pubkey());
        index.insert_spk(1, addr_1.script_pubkey());
        index.insert_spk(2, addr_2.script_pubkey());
        index
    });

    let emitter = &mut Emitter::new(&env.client, 0);

    loop {
        let (block, height) = match emitter.next_block()? {
            Some(b) => (b.block, b.height),
            None => break,
        };
        let _ = chain.apply_update(block_to_chain_update(&block, height))?;
        let indexed_additions =
            indexed_tx_graph.insert_relevant_txs(block_to_tx_graph_update(&block, height));
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

        let mempool_txs = emitter.mempool().collect::<Result<Vec<_>, _>>()?;
        let indexed_additions =
            indexed_tx_graph.insert_relevant_txs(mempool_to_tx_graph_update(&mempool_txs));
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
        let EmittedBlock { block, height } = emitter.next_block()?.expect("must get mined block");
        let _ = chain.apply_update(block_to_chain_update(&block, height))?;
        let indexed_additions =
            indexed_tx_graph.insert_relevant_txs(block_to_tx_graph_update(&block, height));
        assert!(indexed_additions.graph.txs.is_empty());
        assert!(indexed_additions.graph.txouts.is_empty());
        assert_eq!(indexed_additions.graph.anchors, exp_anchors);
    }

    Ok(())
}
