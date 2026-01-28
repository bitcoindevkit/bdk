#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

pub mod utils;

use anyhow::Context;
use bdk_chain::bitcoin::{
    block::Header, hash_types::TxMerkleNode, hex::FromHex, script::PushBytesBuf, transaction,
    Address, Amount, Block, BlockHash, CompactTarget, ScriptBuf, Transaction, TxIn, TxOut, Txid,
};
use bdk_chain::CheckPoint;
use bitcoin::address::NetworkChecked;
use bitcoin::consensus::Decodable;
use bitcoin::hex::HexToBytesError;
use core::str::FromStr;
use core::time::Duration;
use electrsd::corepc_node::vtype::GetBlockTemplate;
use electrsd::corepc_node::{TemplateRequest, TemplateRules};

pub use electrsd;
pub use electrsd::corepc_client;
pub use electrsd::corepc_node;
pub use electrsd::corepc_node::anyhow;
pub use electrsd::electrum_client;
use electrsd::electrum_client::ElectrumApi;

/// Struct for running a regtest environment with a single `bitcoind` node with an `electrs`
/// instance connected to it.
pub struct TestEnv {
    pub bitcoind: electrsd::corepc_node::Node,
    pub electrsd: electrsd::ElectrsD,
}

/// Configuration parameters.
#[derive(Debug)]
pub struct Config<'a> {
    /// [`corepc_node::Conf`]
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

/// Parameters for [`TestEnv::mine_block`].
#[non_exhaustive]
#[derive(Default)]
pub struct MineParams {
    /// If `true`, the block will be empty (no mempool transactions).
    pub empty: bool,
    /// Set a custom block timestamp. Defaults to `max(min_time, now)`.
    pub time: Option<u32>,
    /// Set a custom coinbase output script. Defaults to `OP_TRUE`.
    pub coinbase_address: Option<ScriptBuf>,
}

impl MineParams {
    fn address_or_anyone_can_spend(&self) -> ScriptBuf {
        self.coinbase_address
            .clone()
            // OP_TRUE (anyone can spend)
            .unwrap_or(ScriptBuf::from_bytes(vec![0x51]))
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

    /// Exposes the RPC calls from [`corepc_client`].
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

    /// Get a block template from the node.
    pub fn get_block_template(&self) -> anyhow::Result<GetBlockTemplate> {
        use corepc_node::TemplateRequest;
        Ok(self.bitcoind.client.get_block_template(&TemplateRequest {
            rules: vec![
                TemplateRules::Segwit,
                TemplateRules::Taproot,
                TemplateRules::Csv,
            ],
        })?)
    }

    /// Mine a block that is guaranteed to be empty even with transactions in the mempool.
    #[cfg(feature = "std")]
    pub fn mine_empty_block(&self) -> anyhow::Result<(usize, BlockHash)> {
        self.mine_block(MineParams {
            empty: true,
            ..Default::default()
        })
    }

    /// Get the minimum valid timestamp for the next block.
    pub fn min_time_for_next_block(&self) -> anyhow::Result<u32> {
        Ok(self
            .bitcoind
            .client
            .get_block_template(&TemplateRequest {
                rules: vec![
                    TemplateRules::Segwit,
                    TemplateRules::Taproot,
                    TemplateRules::Csv,
                ],
            })?
            .min_time)
    }

    /// Mine a single block with the given [`MineParams`].
    pub fn mine_block(&self, params: MineParams) -> anyhow::Result<(usize, BlockHash)> {
        let bt = self.bitcoind.client.get_block_template(&TemplateRequest {
            rules: vec![
                TemplateRules::Segwit,
                TemplateRules::Taproot,
                TemplateRules::Csv,
            ],
        })?;
        let coinbase_scriptsig = {
            // BIP34 requires the height to be the first item in coinbase scriptSig.
            // Bitcoin Core validates by checking if scriptSig STARTS with the expected
            // encoding (using minimal opcodes like OP_1 for height 1).
            // The scriptSig must also be 2-100 bytes total.
            let mut builder = bdk_chain::bitcoin::script::Builder::new().push_int(bt.height);
            for v in bt.coinbase_aux.values() {
                let bytes = Vec::<u8>::from_hex(v).expect("must be valid hex");
                let bytes_buf = PushBytesBuf::try_from(bytes).expect("must be valid bytes");
                builder = builder.push_slice(bytes_buf);
            }
            let script = builder.into_script();
            // Ensure scriptSig is at least 2 bytes (pad with OP_0 if needed)
            if script.len() < 2 {
                bdk_chain::bitcoin::script::Builder::new()
                    .push_int(bt.height)
                    .push_opcode(bdk_chain::bitcoin::opcodes::OP_0)
                    .into_script()
            } else {
                script
            }
        };

        let coinbase_outputs = if params.empty {
            let value: Amount = Amount::from_sat(
                (bt.coinbase_value - bt.transactions.iter().map(|tx| tx.fee).sum::<i64>()) as u64,
            );
            vec![TxOut {
                value,
                script_pubkey: params.address_or_anyone_can_spend(),
            }]
        } else {
            core::iter::once(TxOut {
                value: Amount::from_sat(bt.coinbase_value.try_into().expect("must fit into u64")),
                script_pubkey: params.address_or_anyone_can_spend(),
            })
            .chain(
                bt.default_witness_commitment
                    .map(|s| -> Result<_, HexToBytesError> {
                        Ok(TxOut {
                            value: Amount::ZERO,
                            script_pubkey: ScriptBuf::from_hex(&s)?,
                        })
                    })
                    .transpose()?,
            )
            .collect()
        };

        let coinbase_tx = Transaction {
            version: transaction::Version::ONE,
            lock_time: bdk_chain::bitcoin::absolute::LockTime::from_height(0)?,
            input: vec![TxIn {
                previous_output: bdk_chain::bitcoin::OutPoint::default(),
                script_sig: coinbase_scriptsig,
                sequence: bdk_chain::bitcoin::Sequence::default(),
                witness: bdk_chain::bitcoin::Witness::new(),
            }],
            output: coinbase_outputs,
        };

        let txdata = if params.empty {
            vec![coinbase_tx]
        } else {
            core::iter::once(coinbase_tx)
                .chain(bt.transactions.iter().map(|tx| {
                    let raw = Vec::<u8>::from_hex(&tx.data).expect("must be valid hex");
                    Transaction::consensus_decode(&mut raw.as_slice()).expect("must decode tx")
                }))
                .collect()
        };

        let mut block = Block {
            header: Header {
                version: bdk_chain::bitcoin::blockdata::block::Version::from_consensus(bt.version),
                prev_blockhash: BlockHash::from_str(&bt.previous_block_hash)?,
                merkle_root: TxMerkleNode::from_raw_hash(
                    bdk_chain::bitcoin::merkle_tree::calculate_root(
                        txdata.iter().map(|tx| tx.compute_txid().to_raw_hash()),
                    )
                    .expect("must have atleast one tx"),
                ),
                time: params.time.unwrap_or(Ord::max(
                    bt.min_time,
                    std::time::UNIX_EPOCH.elapsed()?.as_secs() as u32,
                )),
                bits: CompactTarget::from_unprefixed_hex(&bt.bits)?,
                nonce: 0,
            },
            txdata,
        };

        block.header.merkle_root = block.compute_merkle_root().expect("must compute");

        // Mine!
        let target = block.header.target();
        for nonce in 0..=u32::MAX {
            block.header.nonce = nonce;
            let blockhash = block.block_hash();
            if target.is_met_by(blockhash) {
                self.rpc_client().submit_block(&block)?;
                return Ok((bt.height as usize, blockhash));
            }
        }

        Err(anyhow::anyhow!("Cannot find nonce that meets the target"))
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
    #[cfg(feature = "std")]
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
    use crate::{MineParams, TestEnv};
    use bdk_chain::bitcoin::{Amount, ScriptBuf};
    use core::time::Duration;
    use electrsd::corepc_node::anyhow::Result;
    use std::collections::BTreeSet;

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

    #[test]
    fn test_mine_block() -> Result<()> {
        let anyone_can_spend = ScriptBuf::from_bytes(vec![0x51]);

        let env = TestEnv::new()?;

        // So we can spend.
        let addr = env
            .rpc_client()
            .get_new_address(None, None)?
            .address()?
            .assume_checked();
        env.mine_blocks(100, Some(addr.clone()))?;

        // Try mining a block with custom time.
        let custom_time = env.min_time_for_next_block()? + 100;
        let (_a_height, a_hash) = env.mine_block(MineParams {
            empty: false,
            time: Some(custom_time),
            coinbase_address: None,
        })?;
        let a_block = env.rpc_client().get_block(a_hash)?;
        assert_eq!(a_block.header.time, custom_time);
        assert_eq!(
            a_block.txdata[0].output[0].script_pubkey, anyone_can_spend,
            "Subsidy address must be anyone_can_spend"
        );

        // Now try mining with min time & some txs.
        let txid1 = env.send(&addr, Amount::from_sat(100_000))?;
        let txid2 = env.send(&addr, Amount::from_sat(200_000))?;
        let txid3 = env.send(&addr, Amount::from_sat(300_000))?;
        let min_time = env.min_time_for_next_block()?;
        let (_b_height, b_hash) = env.mine_block(MineParams {
            empty: false,
            time: Some(min_time),
            coinbase_address: None,
        })?;
        let b_block = env.rpc_client().get_block(b_hash)?;
        assert_eq!(b_block.header.time, min_time);
        assert_eq!(
            a_block.txdata[0].output[0].script_pubkey, anyone_can_spend,
            "Subsidy address must be anyone_can_spend"
        );
        assert_eq!(
            b_block
                .txdata
                .iter()
                .skip(1) // ignore coinbase
                .map(|tx| tx.compute_txid())
                .collect::<BTreeSet<_>>(),
            [txid1, txid2, txid3].into_iter().collect(),
            "Must have all txs"
        );

        // Custom subsidy address.
        let (_c_height, c_hash) = env.mine_block(MineParams {
            empty: false,
            time: None,
            coinbase_address: Some(addr.script_pubkey()),
        })?;
        let c_block = env.rpc_client().get_block(c_hash)?;
        assert_eq!(
            c_block.txdata[0].output[0].script_pubkey,
            addr.script_pubkey(),
            "Custom address works"
        );

        Ok(())
    }
}
