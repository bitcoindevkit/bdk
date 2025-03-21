#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod utils;

use bdk_chain::{
    bitcoin::{
        address::NetworkChecked, block::Header, hash_types::TxMerkleNode, hashes::Hash,
        secp256k1::rand::random, transaction, Address, Amount, Block, BlockHash, ScriptBuf,
        ScriptHash, Transaction, TxIn, TxOut, Txid,
    },
    local_chain::CheckPoint,
};
use electrsd::corepc_node::{anyhow::Context, TemplateRequest, TemplateRules};

pub use electrsd;
pub use electrsd::corepc_client;
pub use electrsd::corepc_node;
pub use electrsd::corepc_node::anyhow;
pub use electrsd::electrum_client;
use electrsd::electrum_client::ElectrumApi;
use std::time::Duration;

/// Struct for running a regtest environment with a single `bitcoind` node with an `electrs`
/// instance connected to it.
pub struct TestEnv {
    pub bitcoind: electrsd::corepc_node::Node,
    pub electrsd: electrsd::ElectrsD,
}

/// Configuration parameters.
#[derive(Debug)]
pub struct Config<'a> {
    /// [`bitcoind::Conf`]
    pub bitcoind: corepc_node::Conf<'a>,
    /// [`electrsd::Conf`]
    pub electrsd: electrsd::Conf<'a>,
}

impl Default for Config<'_> {
    /// Use the default configuration plus set `http_enabled = true` for [`electrsd::Conf`]
    /// which is required for testing `bdk_esplora`.
    fn default() -> Self {
        Self {
            bitcoind: corepc_node::Conf::default(),
            electrsd: {
                let mut conf = electrsd::Conf::default();
                conf.http_enabled = true;
                conf
            },
        }
    }
}

impl TestEnv {
    /// Construct a new [`TestEnv`] instance with the default configuration used by BDK.
    pub fn new() -> anyhow::Result<Self> {
        TestEnv::new_with_config(Config::default())
    }

    /// Construct a new [`TestEnv`] instance with the provided [`Config`].
    pub fn new_with_config(config: Config) -> anyhow::Result<Self> {
        let bitcoind_exe = match std::env::var("BITCOIND_EXE") {
            Ok(path) => path,
            Err(_) => corepc_node::downloaded_exe_path().context(
                "you need to provide an env var BITCOIND_EXE or specify a bitcoind version feature",
            )?,
        };
        let bitcoind = corepc_node::Node::with_conf(bitcoind_exe, &config.bitcoind)?;

        let electrs_exe = match std::env::var("ELECTRS_EXE") {
            Ok(path) => path,
            Err(_) => electrsd::downloaded_exe_path()
                .context("electrs version feature must be enabled")?,
        };
        let electrsd = electrsd::ElectrsD::with_conf(electrs_exe, &bitcoind, &config.electrsd)?;

        Ok(Self { bitcoind, electrsd })
    }

    /// Exposes the [`ElectrumApi`] calls from the Electrum client.
    pub fn electrum_client(&self) -> &impl ElectrumApi {
        &self.electrsd.client
    }

    /// Exposes the [`RpcApi`] calls from [`bitcoincore_rpc`].
    pub fn rpc_client(&self) -> &corepc_node::Client {
        &self.bitcoind.client
    }

    // Reset `electrsd` so that new blocks can be seen.
    pub fn reset_electrsd(mut self) -> anyhow::Result<Self> {
        let mut electrsd_conf = electrsd::Conf::default();
        electrsd_conf.http_enabled = true;
        let electrsd = match std::env::var_os("ELECTRS_EXE") {
            Some(env_electrs_exe) => {
                electrsd::ElectrsD::with_conf(env_electrs_exe, &self.bitcoind, &electrsd_conf)
            }
            None => {
                let electrs_exe = electrsd::downloaded_exe_path()
                    .expect("electrs version feature must be enabled");
                electrsd::ElectrsD::with_conf(electrs_exe, &self.bitcoind, &electrsd_conf)
            }
        }?;
        self.electrsd = electrsd;
        Ok(self)
    }

    /// Mine a number of blocks of a given size `count`, which may be specified to a given coinbase
    /// `address`.
    pub fn mine_blocks(
        &self,
        count: usize,
        address: Option<Address>,
    ) -> anyhow::Result<Vec<BlockHash>> {
        let coinbase_address = match address {
            Some(address) => address,
            None => self.bitcoind.client.new_address()?,
        };
        let block_hashes = self
            .bitcoind
            .client
            .generate_to_address(count as _, &coinbase_address)?
            .into_model()?
            .0;
        Ok(block_hashes)
    }

    /// Mine a block that is guaranteed to be empty even with transactions in the mempool.
    pub fn mine_empty_block(&self) -> anyhow::Result<(usize, BlockHash)> {
        let request = TemplateRequest {
            rules: vec![TemplateRules::Segwit],
        };
        let bt = self
            .bitcoind
            .client
            .get_block_template(&request)?
            .into_model()?;

        let txdata = vec![Transaction {
            version: transaction::Version::ONE,
            lock_time: bdk_chain::bitcoin::absolute::LockTime::from_height(0)?,
            input: vec![TxIn {
                previous_output: bdk_chain::bitcoin::OutPoint::default(),
                script_sig: ScriptBuf::builder()
                    .push_int(bt.height as _)
                    // random number so that re-mining creates unique block
                    .push_int(random())
                    .into_script(),
                sequence: bdk_chain::bitcoin::Sequence::default(),
                witness: bdk_chain::bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new_p2sh(&ScriptHash::all_zeros()),
            }],
        }];

        let mut block = Block {
            header: Header {
                version: bt.version,
                prev_blockhash: bt.previous_block_hash,
                merkle_root: TxMerkleNode::all_zeros(),
                time: Ord::max(
                    bt.min_time,
                    std::time::UNIX_EPOCH.elapsed()?.as_secs() as u32,
                ),
                bits: bt.bits,
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

        self.bitcoind.client.submit_block(&block)?;

        Ok((bt.height as usize, block.block_hash()))
    }

    /// This method waits for the Electrum notification indicating that a new block has been mined.
    /// `timeout` is the maximum [`Duration`] we want to wait for a response from Electrsd.
    pub fn wait_until_electrum_sees_block(&self, timeout: Duration) -> anyhow::Result<()> {
        self.electrsd.client.block_headers_subscribe()?;
        let delay = Duration::from_millis(200);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            self.electrsd.trigger()?;
            self.electrsd.client.ping()?;
            if self.electrsd.client.block_headers_pop()?.is_some() {
                return Ok(());
            }

            std::thread::sleep(delay);
        }

        Err(anyhow::Error::msg(format!(
            "Timed out waiting for Electrsd to get transaction, took: {:?}",
            start.elapsed()
        )))
    }

    /// This method waits for Electrsd to see a transaction with given `txid`. `timeout` is the
    /// maximum [`Duration`] we want to wait for a response from Electrsd.
    pub fn wait_until_electrum_sees_txid(
        &self,
        txid: Txid,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let delay = Duration::from_millis(200);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if self.electrsd.client.transaction_get(&txid).is_ok() {
                return Ok(());
            }

            std::thread::sleep(delay);
        }

        Err(anyhow::Error::msg(format!(
            "Timed out waiting for Electrsd to get transaction, took: {:?}",
            start.elapsed()
        )))
    }

    /// Invalidate a number of blocks of a given size `count`.
    pub fn invalidate_blocks(&self, count: usize) -> anyhow::Result<()> {
        let mut hash = self.bitcoind.client.get_best_block_hash()?.block_hash()?;
        for _ in 0..count {
            let prev_hash = self
                .bitcoind
                .client
                .get_block_verbose_one(hash)?
                .into_model()?
                .previous_block_hash;
            self.bitcoind.client.invalidate_block(hash)?;
            match prev_hash {
                Some(prev_hash) => hash = prev_hash,
                None => break,
            }
        }
        Ok(())
    }

    /// Reorg a number of blocks of a given size `count`.
    /// Refer to [`TestEnv::mine_empty_block`] for more information.
    pub fn reorg(&self, count: usize) -> anyhow::Result<Vec<BlockHash>> {
        let start_height = self.bitcoind.client.get_block_count()?;
        self.invalidate_blocks(count)?;

        let res = self.mine_blocks(count, None);
        assert_eq!(
            self.bitcoind.client.get_block_count()?,
            start_height,
            "reorg should not result in height change"
        );
        res
    }

    /// Reorg with a number of empty blocks of a given size `count`.
    pub fn reorg_empty_blocks(&self, count: usize) -> anyhow::Result<Vec<(usize, BlockHash)>> {
        let start_height = self.bitcoind.client.get_block_count()?;
        self.invalidate_blocks(count)?;

        let res = (0..count)
            .map(|_| self.mine_empty_block())
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            self.bitcoind.client.get_block_count()?,
            start_height,
            "reorg should not result in height change"
        );
        Ok(res)
    }

    /// Send a tx of a given `amount` to a given `address`.
    pub fn send(&self, address: &Address<NetworkChecked>, amount: Amount) -> anyhow::Result<Txid> {
        let txid = self
            .bitcoind
            .client
            .send_to_address(address, amount)?
            .txid()?;
        Ok(txid)
    }

    /// Create a checkpoint linked list of all the blocks in the chain.
    pub fn make_checkpoint_tip(&self) -> CheckPoint<BlockHash> {
        CheckPoint::from_blocks((0_u32..).map_while(|height| {
            self.get_block_hash(height as u64)
                .ok()
                .map(|block_hash| (height, block_hash))
        }))
        .expect("must craft tip")
    }

    /// Get the genesis hash of the blockchain.
    pub fn genesis_hash(&self) -> anyhow::Result<BlockHash> {
        let hash = self.bitcoind.client.get_block_hash(0)?.into_model()?.0;
        Ok(hash)
    }

    /// Get block hash by `height` from the `bitcoind` client.
    pub fn get_block_hash(&self, height: u64) -> anyhow::Result<BlockHash> {
        Ok(self.bitcoind.client.get_block_hash(height)?.block_hash()?)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test {
    use crate::TestEnv;
    use core::time::Duration;
    use electrsd::corepc_node::anyhow::Result;

    /// This checks that reorgs initiated by `bitcoind` is detected by our `electrsd` instance.
    #[test]
    fn test_reorg_is_detected_in_electrsd() -> Result<()> {
        let env = TestEnv::new()?;

        // Mine some blocks.
        env.mine_blocks(101, None)?;
        env.wait_until_electrum_sees_block(Duration::from_secs(6))?;
        let height = env.bitcoind.client.get_block_count()?.into_model().0;
        let blocks = (0..=height)
            .map(|i| env.bitcoind.client.get_block_hash(i))
            .collect::<Result<Vec<_>, _>>()?;

        // Perform reorg on six blocks.
        env.reorg(6)?;
        env.wait_until_electrum_sees_block(Duration::from_secs(6))?;
        let reorged_height = env.bitcoind.client.get_block_count()?.into_model().0;
        let reorged_blocks = (0..=height)
            .map(|i| env.bitcoind.client.get_block_hash(i))
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(height, reorged_height);

        // Block hashes should not be equal on the six reorged blocks.
        for (i, (block, reorged_block)) in blocks.iter().zip(reorged_blocks.iter()).enumerate() {
            match i <= height as usize - 6 {
                true => assert_eq!(block, reorged_block),
                false => assert_ne!(block, reorged_block),
            }
        }

        Ok(())
    }
}
