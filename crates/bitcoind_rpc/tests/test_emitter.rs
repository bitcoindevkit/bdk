use std::collections::{BTreeMap, BTreeSet};

use bdk_bitcoind_rpc::Emitter;
use bdk_chain::{
    bitcoin::{Address, Amount, BlockHash, Txid},
    local_chain::LocalChain,
    Append, BlockId, ConfirmationHeightAnchor, IndexedTxGraph, SpkTxOutIndex,
};
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
    let mut emitter = Emitter::new(&env.client, 0, local_chain.tip());

    // mine some blocks and returned the actual block hashes
    let exp_hashes = {
        let mut hashes = vec![env.client.get_block_hash(0)?]; // include genesis block
        hashes.extend(env.mine_blocks(101, None)?);
        hashes
    };

    // see if the emitter outputs the right blocks
    loop {
        let cp = match emitter.emit_block()? {
            Some(b) => b.checkpoint(),
            None => break,
        };
        assert_eq!(
            cp.hash(),
            exp_hashes[cp.height() as usize],
            "emitted block hash is unexpected"
        );

        let chain_update = bdk_chain::local_chain::Update {
            tip: cp.clone(),
            introduce_older_blocks: false,
        };
        assert_eq!(
            local_chain.apply_update(chain_update)?,
            BTreeMap::from([(cp.height(), Some(cp.hash()))]),
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

    // create new emitter (just for testing sake)
    drop(emitter);
    let mut emitter = Emitter::new(&env.client, 0, local_chain.tip());

    // perform reorg
    let reorged_blocks = env.reorg(6)?;
    let exp_hashes = exp_hashes
        .iter()
        .take(exp_hashes.len() - reorged_blocks.len())
        .chain(&reorged_blocks)
        .cloned()
        .collect::<Vec<_>>();

    // see if the emitter outputs the right blocks
    let mut exp_height = exp_hashes.len() - reorged_blocks.len();
    loop {
        let cp = match emitter.emit_block()? {
            Some(b) => b.checkpoint(),
            None => break,
        };
        assert_eq!(
            cp.height(),
            exp_height as u32,
            "emitted block has unexpected height"
        );

        assert_eq!(
            cp.hash(),
            exp_hashes[cp.height() as usize],
            "emitted block is unexpected"
        );

        let chain_update = bdk_chain::local_chain::Update {
            tip: cp.clone(),
            introduce_older_blocks: false,
        };
        assert_eq!(
            local_chain.apply_update(chain_update)?,
            if exp_height == exp_hashes.len() - reorged_blocks.len() {
                core::iter::once((cp.height(), Some(cp.hash())))
                    .chain((cp.height() + 1..exp_hashes.len() as u32).map(|h| (h, None)))
                    .collect::<bdk_chain::local_chain::ChangeSet>()
            } else {
                BTreeMap::from([(cp.height(), Some(cp.hash()))])
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
    let mut indexed_tx_graph = IndexedTxGraph::<ConfirmationHeightAnchor, _>::new({
        let mut index = SpkTxOutIndex::<usize>::default();
        index.insert_spk(0, addr_0.script_pubkey());
        index.insert_spk(1, addr_1.script_pubkey());
        index.insert_spk(2, addr_2.script_pubkey());
        index
    });

    for r in Emitter::new(&env.client, 0, chain.tip()) {
        let update = r?;

        if let Some(chain_update) = update.chain_update() {
            let _ = chain.apply_update(chain_update)?;
        }

        let tx_graph_update =
            update.indexed_tx_graph_update(bdk_bitcoind_rpc::confirmation_height_anchor);

        let indexed_additions = indexed_tx_graph.insert_relevant_txs(tx_graph_update);
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

    // expect the next update to be a mempool update (with 3 relevant tx)
    {
        let update = Emitter::new(&env.client, 0, chain.tip()).emit_update()?;
        assert!(update.is_mempool());

        let tx_graph_update =
            update.indexed_tx_graph_update(bdk_bitcoind_rpc::confirmation_height_anchor);

        let indexed_additions = indexed_tx_graph.insert_relevant_txs(tx_graph_update);
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
            let anchor = ConfirmationHeightAnchor {
                anchor_block: BlockId {
                    height: exp_block_height,
                    hash: exp_block_hash,
                },
                confirmation_height: exp_block_height,
            };
            move |&txid| (anchor, txid)
        })
        .collect::<BTreeSet<_>>();

    {
        let update = Emitter::new(&env.client, 0, chain.tip()).emit_update()?;
        assert!(update.is_block());

        if let Some(chain_update) = update.chain_update() {
            let _ = chain.apply_update(chain_update)?;
        }

        let tx_graph_update =
            update.indexed_tx_graph_update(bdk_bitcoind_rpc::confirmation_height_anchor);

        let indexed_additions = indexed_tx_graph.insert_relevant_txs(tx_graph_update);
        assert!(indexed_additions.graph.txs.is_empty());
        assert!(indexed_additions.graph.txouts.is_empty());
        assert_eq!(indexed_additions.graph.anchors, exp_anchors);
    }

    Ok(())
}
