use crate::testutils::TestIncomingTx;
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::sha256d;
use bitcoin::{Address, Amount, Script, Transaction, Txid};
pub use bitcoincore_rpc::bitcoincore_rpc_json::AddressType;
pub use bitcoincore_rpc::{Auth, Client as RpcClient, RpcApi};
use core::str::FromStr;
pub use electrum_client::{Client as ElectrumClient, ElectrumApi};
#[allow(unused_imports)]
use log::{debug, error, info, trace};
use std::collections::HashMap;
use std::env;
use std::ops::Deref;
use std::path::PathBuf;
use std::time::Duration;

pub struct TestClient {
    client: RpcClient,
    electrum: ElectrumClient,
}

impl TestClient {
    pub fn new(rpc_host_and_wallet: String, rpc_wallet_name: String) -> Self {
        let client = RpcClient::new(
            format!("http://{}/wallet/{}", rpc_host_and_wallet, rpc_wallet_name),
            get_auth(),
        )
        .unwrap();
        let electrum = ElectrumClient::new(&get_electrum_url()).unwrap();

        TestClient { client, electrum }
    }

    fn wait_for_tx(&mut self, txid: Txid, monitor_script: &Script) {
        // wait for electrs to index the tx
        exponential_backoff_poll(|| {
            trace!("wait_for_tx {}", txid);

            self.electrum
                .script_get_history(monitor_script)
                .unwrap()
                .iter()
                .position(|entry| entry.tx_hash == txid)
        });
    }

    fn wait_for_block(&mut self, min_height: usize) {
        self.electrum.block_headers_subscribe().unwrap();

        loop {
            let header = exponential_backoff_poll(|| {
                self.electrum.ping().unwrap();
                self.electrum.block_headers_pop().unwrap()
            });
            if header.height >= min_height {
                break;
            }
        }
    }

    pub fn receive(&mut self, meta_tx: TestIncomingTx) -> Txid {
        assert!(
            !meta_tx.output.is_empty(),
            "can't create a transaction with no outputs"
        );

        let mut map = HashMap::new();

        let mut required_balance = 0;
        for out in &meta_tx.output {
            required_balance += out.value;
            map.insert(out.to_address.clone(), Amount::from_sat(out.value));
        }

        if self.get_balance(None, None).unwrap() < Amount::from_sat(required_balance) {
            panic!("Insufficient funds in bitcoind. Please generate a few blocks with: `bitcoin-cli generatetoaddress 10 {}`", self.get_new_address(None, None).unwrap());
        }

        // FIXME: core can't create a tx with two outputs to the same address
        let tx = self
            .create_raw_transaction_hex(&[], &map, meta_tx.locktime, meta_tx.replaceable)
            .unwrap();
        let tx = self.fund_raw_transaction(tx, None, None).unwrap();
        let mut tx: Transaction = deserialize(&tx.hex).unwrap();

        if let Some(true) = meta_tx.replaceable {
            // for some reason core doesn't set this field right
            for input in &mut tx.input {
                input.sequence = 0xFFFFFFFD;
            }
        }

        let tx = self
            .sign_raw_transaction_with_wallet(&serialize(&tx), None, None)
            .unwrap();

        // broadcast through electrum so that it caches the tx immediately
        let txid = self
            .electrum
            .transaction_broadcast(&deserialize(&tx.hex).unwrap())
            .unwrap();

        if let Some(num) = meta_tx.min_confirmations {
            self.generate(num, None);
        }

        let monitor_script = Address::from_str(&meta_tx.output[0].to_address)
            .unwrap()
            .script_pubkey();
        self.wait_for_tx(txid, &monitor_script);

        debug!("Sent tx: {}", txid);

        txid
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

        let monitor_script =
            tx.vout[0].script_pub_key.addresses.as_ref().unwrap()[0].script_pubkey();
        self.wait_for_tx(new_txid, &monitor_script);

        debug!("Bumped {}, new txid {}", txid, new_txid);

        new_txid
    }

    pub fn generate_manually(&mut self, txs: Vec<Transaction>) -> String {
        use bitcoin::blockdata::block::{Block, BlockHeader};
        use bitcoin::blockdata::script::Builder;
        use bitcoin::blockdata::transaction::{OutPoint, TxIn, TxOut};
        use bitcoin::hash_types::{BlockHash, TxMerkleNode};

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
            merkle_root: TxMerkleNode::default(),
            time: block_template["curtime"].as_u64().unwrap() as u32,
            bits: u32::from_str_radix(block_template["bits"].as_str().unwrap(), 16).unwrap(),
            nonce: 0,
        };
        debug!("header: {:#?}", header);

        let height = block_template["height"].as_u64().unwrap() as i64;
        let witness_reserved_value: Vec<u8> = sha256d::Hash::default().as_ref().into();
        // burn block subsidy and fees, not a big deal
        let mut coinbase_tx = Transaction {
            version: 1,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: Builder::new().push_int(height).into_script(),
                sequence: 0xFFFFFFFF,
                witness: vec![witness_reserved_value],
            }],
            output: vec![],
        };

        let mut txdata = vec![coinbase_tx.clone()];
        txdata.extend_from_slice(&txs);

        let mut block = Block { header, txdata };

        let witness_root = block.witness_root();
        let witness_commitment =
            Block::compute_witness_commitment(&witness_root, &coinbase_tx.input[0].witness[0]);

        // now update and replace the coinbase tx
        let mut coinbase_witness_commitment_script = vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed];
        coinbase_witness_commitment_script.extend_from_slice(&witness_commitment);

        coinbase_tx.output.push(TxOut {
            value: 0,
            script_pubkey: coinbase_witness_commitment_script.into(),
        });
        block.txdata[0] = coinbase_tx;

        // set merkle root
        let merkle_root = block.merkle_root();
        block.header.merkle_root = merkle_root;

        assert!(block.check_merkle_root());
        assert!(block.check_witness_commitment());

        // now do PoW :)
        let target = block.header.target();
        while block.header.validate_pow(&target).is_err() {
            block.header.nonce = block.header.nonce.checked_add(1).unwrap(); // panic if we run out of nonces
        }

        let block_hex: String = serialize(&block).to_hex();
        debug!("generated block hex: {}", block_hex);

        self.electrum.block_headers_subscribe().unwrap();

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

    pub fn generate(&mut self, num_blocks: u64, address: Option<Address>) {
        let address = address.unwrap_or_else(|| self.get_new_address(None, None).unwrap());
        let hashes = self.generate_to_address(num_blocks, &address).unwrap();
        let best_hash = hashes.last().unwrap();
        let height = self.get_block_info(best_hash).unwrap().height;

        self.wait_for_block(height);

        debug!("Generated blocks to new height {}", height);
    }

    pub fn invalidate(&mut self, num_blocks: u64) {
        self.electrum.block_headers_subscribe().unwrap();

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
        &self.client
    }
}

impl Default for TestClient {
    fn default() -> Self {
        let rpc_host_and_port =
            env::var("BDK_RPC_URL").unwrap_or_else(|_| "127.0.0.1:18443".to_string());
        let wallet = env::var("BDK_RPC_WALLET").unwrap_or_else(|_| "bdk-test".to_string());
        Self::new(rpc_host_and_port, wallet)
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

// TODO: we currently only support env vars, we could also parse a toml file
fn get_auth() -> Auth {
    match env::var("BDK_RPC_AUTH").as_ref().map(String::as_ref) {
        Ok("USER_PASS") => Auth::UserPass(
            env::var("BDK_RPC_USER").unwrap(),
            env::var("BDK_RPC_PASS").unwrap(),
        ),
        _ => Auth::CookieFile(PathBuf::from(
            env::var("BDK_RPC_COOKIEFILE")
                .unwrap_or_else(|_| "/home/user/.bitcoin/regtest/.cookie".to_string()),
        )),
    }
}

/// This macro runs blockchain tests against a `Blockchain` implementation. It requires access to a
/// Bitcoin core wallet via RPC. At the moment you have to dig into the code yourself and look at
/// the setup required to run the tests yourself.
#[macro_export]
macro_rules! bdk_blockchain_tests {
    (
     fn test_instance() -> $blockchain:ty $block:block) => {
        #[cfg(test)]
        mod bdk_blockchain_tests {
            use $crate::bitcoin::Network;
            use $crate::testutils::blockchain_tests::TestClient;
            use $crate::blockchain::noop_progress;
            use $crate::database::MemoryDatabase;
            use $crate::types::KeychainKind;
            use $crate::{Wallet, FeeRate};
            use $crate::wallet::AddressIndex::New;
            use $crate::testutils;
            use $crate::serial_test::serial;

            use super::*;

            fn get_blockchain() -> $blockchain {
                $block
            }

            fn get_wallet_from_descriptors(descriptors: &(String, Option<String>)) -> Wallet<$blockchain, MemoryDatabase> {
                Wallet::new(&descriptors.0.to_string(), descriptors.1.as_ref(), Network::Regtest, MemoryDatabase::new(), get_blockchain()).unwrap()
            }

            fn init_single_sig() -> (Wallet<$blockchain, MemoryDatabase>, (String, Option<String>), TestClient) {
                let _ = env_logger::try_init();

                let descriptors = testutils! {
                    @descriptors ( "wpkh(Alice)" ) ( "wpkh(Alice)" ) ( @keys ( "Alice" => (@generate_xprv "/44'/0'/0'/0/*", "/44'/0'/0'/1/*") ) )
                };

                let test_client = TestClient::default();
                let wallet = get_wallet_from_descriptors(&descriptors);

                (wallet, descriptors, test_client)
            }

            #[test]
            #[serial]
            fn test_sync_simple() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let tx = testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                };
                println!("{:?}", tx);
                let txid = test_client.receive(tx);

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_unspent().unwrap()[0].keychain, KeychainKind::External);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.received, 50_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);
            }

            #[test]
            #[serial]
            fn test_sync_stop_gap_20() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 5) => 50_000 )
                });
                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 25) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 100_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
            }

            #[test]
            #[serial]
            fn test_sync_before_and_after_receive() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 0);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
            }

            #[test]
            #[serial]
            fn test_sync_multiple_outputs_same_tx() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000, (@external descriptors, 5) => 30_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 105_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 3);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.received, 105_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);
            }

            #[test]
            #[serial]
            fn test_sync_receive_multi() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });
                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 5) => 25_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 75_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
                assert_eq!(wallet.list_unspent().unwrap().len(), 2);
            }

            #[test]
            #[serial]
            fn test_sync_address_reuse() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 25_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 75_000);
            }

            #[test]
            #[serial]
            fn test_sync_receive_rbf_replaced() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) ( @replaceable true )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.received, 50_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);

                let new_txid = test_client.bump_fee(&txid);

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, new_txid);
                assert_eq!(list_tx_item.received, 50_000);
                assert_eq!(list_tx_item.sent, 0);
                assert_eq!(list_tx_item.height, None);
            }

            // FIXME: I would like this to be cfg_attr(not(feature = "test-esplora"), ignore) but it
            // doesn't work for some reason.
            #[cfg(not(feature = "esplora"))]
            #[test]
            #[serial]
            fn test_sync_reorg_block() {
                let (wallet, descriptors, mut test_client) = init_single_sig();

                let txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) ( @confirmations 1 ) ( @replaceable true )
                });

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);
                assert_eq!(wallet.list_transactions(false).unwrap().len(), 1);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert!(list_tx_item.height.is_some());

                // Invalidate 1 block
                test_client.invalidate(1);

                wallet.sync(noop_progress(), None).unwrap();

                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
                assert_eq!(list_tx_item.txid, txid);
                assert_eq!(list_tx_item.height, None);
            }

            #[test]
            #[serial]
            fn test_sync_after_send() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                println!("{}", descriptors.0);
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey(), 25_000);
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                let tx = psbt.extract_tx();
                println!("{}", bitcoin::consensus::encode::serialize_hex(&tx));
                wallet.broadcast(tx).unwrap();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                assert_eq!(wallet.list_transactions(false).unwrap().len(), 2);
                assert_eq!(wallet.list_unspent().unwrap().len(), 1);
            }

            #[test]
            #[serial]
            fn test_sync_outgoing_from_scratch() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                let received_txid = test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey(), 25_000);
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                let sent_txid = wallet.broadcast(psbt.extract_tx()).unwrap();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                // empty wallet
                let wallet = get_wallet_from_descriptors(&descriptors);
                wallet.sync(noop_progress(), None).unwrap();

                let tx_map = wallet.list_transactions(false).unwrap().into_iter().map(|tx| (tx.txid, tx)).collect::<std::collections::HashMap<_, _>>();

                let received = tx_map.get(&received_txid).unwrap();
                assert_eq!(received.received, 50_000);
                assert_eq!(received.sent, 0);

                let sent = tx_map.get(&sent_txid).unwrap();
                assert_eq!(sent.received, details.received);
                assert_eq!(sent.sent, details.sent);
                assert_eq!(sent.fees, details.fees);
            }

            #[test]
            #[serial]
            fn test_sync_long_change_chain() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 )
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut total_sent = 0;
                for _ in 0..5 {
                    let mut builder = wallet.build_tx();
                    builder.add_recipient(node_addr.script_pubkey(), 5_000);
                    let (mut psbt, details) = builder.finish().unwrap();
                    let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                    assert!(finalized, "Cannot finalize transaction");
                    wallet.broadcast(psbt.extract_tx()).unwrap();

                    wallet.sync(noop_progress(), None).unwrap();

                    total_sent += 5_000 + details.fees;
                }

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - total_sent);

                // empty wallet
                let wallet = get_wallet_from_descriptors(&descriptors);
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - total_sent);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 5_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - details.fees - 5_000);
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(2.1));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000 - new_details.fees - 5_000);
                assert_eq!(wallet.get_balance().unwrap(), new_details.received);

                assert!(new_details.fees > details.fees);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee_remove_change() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 50_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 49_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 1_000 - details.fees);
                assert_eq!(wallet.get_balance().unwrap(), details.received);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(5.0));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 0);
                assert_eq!(new_details.received, 0);

                assert!(new_details.fees > details.fees);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee_add_input() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 75_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 49_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 26_000 - details.fees);
                assert_eq!(details.received, 1_000 - details.fees);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(10.0));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(new_details.sent, 75_000);
                assert_eq!(wallet.get_balance().unwrap(), new_details.received);
            }

            #[test]
            #[serial]
            fn test_sync_bump_fee_add_input_no_change() {
                let (wallet, descriptors, mut test_client) = init_single_sig();
                let node_addr = test_client.get_node_address(None);

                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
                });

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 75_000);

                let mut builder = wallet.build_tx();
                builder.add_recipient(node_addr.script_pubkey().clone(), 49_000).enable_rbf();
                let (mut psbt, details) = builder.finish().unwrap();
                let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 26_000 - details.fees);
                assert_eq!(details.received, 1_000 - details.fees);

                let mut builder = wallet.build_fee_bump(details.txid).unwrap();
                builder.fee_rate(FeeRate::from_sat_per_vb(123.0));
                let (mut new_psbt, new_details) = builder.finish().unwrap();
                println!("{:#?}", new_details);

                let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
                assert!(finalized, "Cannot finalize transaction");
                wallet.broadcast(new_psbt.extract_tx()).unwrap();
                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(new_details.sent, 75_000);
                assert_eq!(wallet.get_balance().unwrap(), 0);
                assert_eq!(new_details.received, 0);
            }

            #[test]
            #[serial]
            fn test_sync_receive_coinbase() {
                let (wallet, _, mut test_client) = init_single_sig();
                let wallet_addr = wallet.get_address(New).unwrap();

                wallet.sync(noop_progress(), None).unwrap();
                assert_eq!(wallet.get_balance().unwrap(), 0);

                test_client.generate(1, Some(wallet_addr));

                wallet.sync(noop_progress(), None).unwrap();
                assert!(wallet.get_balance().unwrap() > 0);
            }
        }
    }
}
