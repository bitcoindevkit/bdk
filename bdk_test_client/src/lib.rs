use bitcoin::consensus::encode::serialize;
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::sha256d;
use bitcoin::{Address, PackedLockTime, Script, Sequence, Transaction, Txid, Witness};
pub use bitcoincore_rpc::bitcoincore_rpc_json::AddressType;
use bitcoincore_rpc::jsonrpc::serde_json::{self, json};
pub use bitcoincore_rpc::{Auth, Client as RpcClient, Error as RpcError, RpcApi};
use core::str::FromStr;
use electrsd::bitcoind::BitcoinD;
use electrsd::{bitcoind, ElectrsD};
pub use electrum_client::{Client as ElectrumClient, ElectrumApi};
#[allow(unused_imports)]
use log::{debug, error, info, log_enabled, trace, Level};
use std::env;
use std::ops::Deref;
use std::time::Duration;

pub struct TestClient {
    pub bitcoind: BitcoinD,
    pub electrsd: ElectrsD,
}

impl TestClient {
    pub fn new(bitcoind_exe: String, electrs_exe: String) -> Self {
        debug!("launching {} and {}", &bitcoind_exe, &electrs_exe);

        let mut conf = bitcoind::Conf::default();
        conf.view_stdout = log_enabled!(Level::Debug);
        let bitcoind = BitcoinD::with_conf(bitcoind_exe, &conf).unwrap();

        let mut conf = electrsd::Conf::default();
        conf.view_stderr = log_enabled!(Level::Debug);
        conf.http_enabled = cfg!(feature = "esplora");

        let electrsd = ElectrsD::with_conf(electrs_exe, &bitcoind, &conf).unwrap();

        let node_address = bitcoind.client.get_new_address(None, None).unwrap();
        bitcoind
            .client
            .generate_to_address(101, &node_address)
            .unwrap();

        let mut test_client = TestClient { bitcoind, electrsd };
        TestClient::wait_for_block(&mut test_client, 101);
        test_client
    }

    fn wait_for_tx(&mut self, txid: Txid, monitor_script: &Script) {
        // wait for electrs to index the tx
        exponential_backoff_poll(|| {
            self.electrsd.trigger().unwrap();
            trace!("wait_for_tx {}", txid);

            self.electrsd
                .client
                .script_get_history(monitor_script)
                .unwrap()
                .iter()
                .position(|entry| entry.tx_hash == txid)
        });
    }

    fn wait_for_block(&mut self, min_height: usize) {
        self.electrsd.client.block_headers_subscribe().unwrap();

        loop {
            let header = exponential_backoff_poll(|| {
                self.electrsd.trigger().unwrap();
                self.electrsd.client.ping().unwrap();
                self.electrsd.client.block_headers_pop().unwrap()
            });
            if header.height >= min_height {
                break;
            }
        }
    }

    pub fn bump_fee(&mut self, txid: &Txid) -> Txid {
        let tx = self.get_raw_transaction_info(txid, None).unwrap();
        assert!(
            tx.confirmations.is_none(),
            "Can't bump tx {} because it's already confirmed",
            txid
        );

        let bumped: serde_json::Value = self.call("bumpfee", &[txid.to_string().into()]).unwrap();
        let new_txid = Txid::from_str(&bumped["txid"].as_str().unwrap().to_string()).unwrap();
        let monitor_script = Script::from_hex(&mut tx.vout[0].script_pub_key.hex.to_hex()).unwrap();
        self.wait_for_tx(new_txid, &monitor_script);

        debug!("Bumped {}, new txid {}", txid, new_txid);

        new_txid
    }

    pub fn generate_manually(&mut self, txs: Vec<Transaction>) -> String {
        use bitcoin::blockdata::block::{Block, BlockHeader};
        use bitcoin::blockdata::script::Builder;
        use bitcoin::blockdata::transaction::{OutPoint, TxIn, TxOut};
        use bitcoin::hash_types::{BlockHash, TxMerkleNode};
        use bitcoin::hashes::Hash;

        let block_template: serde_json::Value = self
            .call("getblocktemplate", &[json!({"rules": ["segwit"]})])
            .unwrap();
        trace!("getblocktemplate: {:#?}", block_template);

        let header = BlockHeader {
            version: block_template["version"].as_i64().unwrap() as i32,
            prev_blockhash: BlockHash::from_hex(
                block_template["previousblockhash"].as_str().unwrap(),
            )
            .unwrap(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: block_template["curtime"].as_u64().unwrap() as u32,
            bits: u32::from_str_radix(block_template["bits"].as_str().unwrap(), 16).unwrap(),
            nonce: 0,
        };
        debug!("header: {:#?}", header);

        let height = block_template["height"].as_u64().unwrap() as i64;
        let witness_reserved_value: Vec<u8> = sha256d::Hash::all_zeros().as_ref().into();
        // burn block subsidy and fees, not a big deal
        let mut coinbase_tx = Transaction {
            version: 1,
            lock_time: PackedLockTime(0),
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: Builder::new().push_int(height).into_script(),
                sequence: Sequence(0xFFFFFFFF),
                witness: Witness::from_vec(vec![witness_reserved_value]),
            }],
            output: vec![],
        };

        let mut txdata = vec![coinbase_tx.clone()];
        txdata.extend_from_slice(&txs);

        let mut block = Block { header, txdata };

        if let Some(witness_root) = block.witness_root() {
            let witness_commitment = Block::compute_witness_commitment(
                &witness_root,
                &coinbase_tx.input[0]
                    .witness
                    .last()
                    .expect("Should contain the witness reserved value"),
            );

            // now update and replace the coinbase tx
            let mut coinbase_witness_commitment_script = vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed];
            coinbase_witness_commitment_script.extend_from_slice(&witness_commitment);

            coinbase_tx.output.push(TxOut {
                value: 0,
                script_pubkey: coinbase_witness_commitment_script.into(),
            });
        }

        block.txdata[0] = coinbase_tx;

        // set merkle root
        if let Some(merkle_root) = block.compute_merkle_root() {
            block.header.merkle_root = merkle_root;
        }

        assert!(block.check_merkle_root());
        assert!(block.check_witness_commitment());

        // now do PoW :)
        let target = block.header.target();
        while block.header.validate_pow(&target).is_err() {
            block.header.nonce = block.header.nonce.checked_add(1).unwrap(); // panic if we run out of nonces
        }

        let block_hex: String = serialize(&block).to_hex();
        debug!("generated block hex: {}", block_hex);

        self.electrsd.client.block_headers_subscribe().unwrap();

        let submit_result: serde_json::Value =
            self.call("submitblock", &[block_hex.into()]).unwrap();
        debug!("submitblock: {:?}", submit_result);
        assert!(
            submit_result.is_null(),
            "submitblock error: {:?}",
            submit_result.as_str()
        );

        self.wait_for_block(height as usize);

        block.header.block_hash().to_hex()
    }

    pub fn generate(&mut self, num_blocks: u64, address: Option<Address>) -> u32 {
        let address = address.unwrap_or_else(|| self.get_new_address(None, None).unwrap());
        let hashes = self.generate_to_address(num_blocks, &address).unwrap();
        let best_hash = hashes.last().unwrap();
        let height = self.get_block_info(best_hash).unwrap().height;

        self.wait_for_block(height);

        debug!("Generated blocks to new height {}", height);
        height as u32
    }

    pub fn invalidate(&mut self, num_blocks: u64) {
        self.electrsd.client.block_headers_subscribe().unwrap();

        let best_hash = self.get_best_block_hash().unwrap();
        let initial_height = self.get_block_info(&best_hash).unwrap().height;

        let mut to_invalidate = best_hash;
        for i in 1..=num_blocks {
            trace!(
                "Invalidating block {}/{} ({})",
                i,
                num_blocks,
                to_invalidate
            );

            self.invalidate_block(&to_invalidate).unwrap();
            to_invalidate = self.get_best_block_hash().unwrap();
        }

        self.wait_for_block(initial_height - num_blocks as usize);

        debug!(
            "Invalidated {} blocks to new height of {}",
            num_blocks,
            initial_height - num_blocks as usize
        );
    }

    pub fn reorg(&mut self, num_blocks: u64) {
        self.invalidate(num_blocks);
        self.generate(num_blocks, None);
    }

    pub fn get_node_address(&self, address_type: Option<AddressType>) -> Address {
        Address::from_str(
            &self
                .get_new_address(None, address_type)
                .unwrap()
                .to_string(),
        )
        .unwrap()
    }
}

pub fn get_electrum_url() -> String {
    env::var("BDK_ELECTRUM_URL").unwrap_or_else(|_| "tcp://127.0.0.1:50001".to_string())
}

impl Deref for TestClient {
    type Target = RpcClient;

    fn deref(&self) -> &Self::Target {
        &self.bitcoind.client
    }
}

impl Default for TestClient {
    fn default() -> Self {
        let bitcoind_exe = env::var("BITCOIND_EXE")
            .ok()
            .or(bitcoind::downloaded_exe_path().ok())
            .expect(
                "you should provide env var BITCOIND_EXE or specifiy a bitcoind version feature",
            );
        let electrs_exe = env::var("ELECTRS_EXE")
            .ok()
            .or(electrsd::downloaded_exe_path())
            .expect(
                "you should provide env var ELECTRS_EXE or specifiy a electrsd version feature",
            );
        Self::new(bitcoind_exe, electrs_exe)
    }
}

fn exponential_backoff_poll<T, F>(mut poll: F) -> T
where
    F: FnMut() -> Option<T>,
{
    let mut delay = Duration::from_millis(64);
    loop {
        match poll() {
            Some(data) => break data,
            None if delay.as_millis() < 512 => delay = delay.mul_f32(2.0),
            None => {}
        }

        std::thread::sleep(delay);
    }
}
