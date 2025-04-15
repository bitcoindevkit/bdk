pub mod utils;

use bdk_chain::{
    bitcoin::{
        address::NetworkChecked, block::Header, hash_types::TxMerkleNode, hashes::Hash,
        secp256k1::rand::random, transaction, Address, Amount, Block, BlockHash, CompactTarget,
        ScriptBuf, ScriptHash, Transaction, TxIn, TxOut, Txid,
    },
    local_chain::CheckPoint,
    BlockId,
};
use electrsd::corepc_node::anyhow::Context;

pub use electrsd;
pub use electrsd::corepc_client;
pub use electrsd::corepc_node;
pub use electrsd::corepc_node::anyhow;
pub use electrsd::electrum_client;
use electrsd::electrum_client::ElectrumApi;
use serde::{Deserialize, Serialize};
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

/// A minimal model of the result of "getblocktemplate", with only used fields
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct GetBlockTemplateResult {
    /// The compressed difficulty in hexadecimal
    pub bits: String,
    /// The previous block hash the current template is mining on
    #[serde(rename = "hash")]
    pub previous_block_hash: bdk_chain::bitcoin::BlockHash,
    /// The height of the block we will be mining: `current height + 1`
    pub height: u64,
    /// The minimum timestamp appropriate for the next block time. Expressed as
    /// UNIX timestamp.
    #[serde(rename = "mintime")]
    pub min_time: u64,
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

    fn get_block_template(&self) -> anyhow::Result<GetBlockTemplateResult> {
        let argument = r#"{"mode": "template", "rules": "segwit", "capabilities": []}"#;
        Ok(self.bitcoind.client.call::<GetBlockTemplateResult>(
            "getblocktemplate",
            &[corepc_node::serde_json::to_value(argument)?],
        )?)
    }

    /// Mine a block that is guaranteed to be empty even with transactions in the mempool.
    pub fn mine_empty_block(&self) -> anyhow::Result<(usize, BlockHash)> {
        let bt = self.get_block_template()?;

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

        let bits: [u8; 4] =
            bdk_chain::bitcoin::consensus::encode::deserialize_hex::<Vec<u8>>(&bt.bits)?
                .clone()
                .try_into()
                .expect("rpc provided us with invalid bits");

        let mut block = Block {
            header: Header {
                version: bdk_chain::bitcoin::block::Version::default(),
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

        let block_hex: String = bdk_chain::bitcoin::consensus::encode::serialize_hex(&block);
        match self.bitcoind.client.call(
            "submitblock",
            &[corepc_node::serde_json::to_value(block_hex)?],
        ) {
            Ok(corepc_node::serde_json::Value::Null) => Ok(()),
            Ok(res) => Err(corepc_client::client_sync::Error::Returned(res.to_string())),
            Err(err) => Err(err.into()),
        }?;
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

        Err(anyhow::Error::msg(
            "Timed out waiting for Electrsd to get block header",
        ))
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

        Err(anyhow::Error::msg(
            "Timed out waiting for Electrsd to get transaction",
        ))
    }

    /// Invalidate a number of blocks of a given size `count`.
    pub fn invalidate_blocks(&self, count: usize) -> anyhow::Result<()> {
        let mut hash = self.bitcoind.client.get_best_block_hash()?.block_hash()?;
        for _ in 0..count {
            let prev_hash = self.bitcoind.client.get_block(hash)?.header.prev_blockhash;
            self.bitcoind.client.invalidate_block(hash)?;
            hash = prev_hash
            // TODO: (@leonardo) It requires a double check if there is any side-effect with this break removal.
            // match prev_hash {
            //     Some(prev_hash) => hash = prev_hash,
            //     None => break,
            // }
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
    pub fn make_checkpoint_tip(&self) -> CheckPoint {
        CheckPoint::from_block_ids((0_u32..).map_while(|height| {
            self.bitcoind
                .client
                .get_block_hash(height as u64)
                .ok()
                .map(|get_block_hash| {
                    let hash = get_block_hash
                        .block_hash()
                        .expect("should `successfully convert to `BlockHash` from `GetBlockHash`");
                    BlockId { height, hash }
                })
        }))
        .expect("must craft tip")
    }

    /// Get the genesis hash of the blockchain.
    pub fn genesis_hash(&self) -> anyhow::Result<BlockHash> {
        let hash = self.bitcoind.client.get_block_hash(0)?.into_model()?.0;
        Ok(hash)
    }
}

#[cfg(test)]
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
