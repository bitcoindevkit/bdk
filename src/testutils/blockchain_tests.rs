use crate::testutils::TestIncomingTx;
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::sha256d;
use bitcoin::{Address, Amount, Script, Transaction, Txid, Witness};
pub use bitcoincore_rpc::bitcoincore_rpc_json::AddressType;
pub use bitcoincore_rpc::{Client as RpcClient, RpcApi};
use core::str::FromStr;
use electrsd::bitcoind::BitcoinD;
use electrsd::{bitcoind, ElectrsD};
pub use electrum_client::{Client as ElectrumClient, ElectrumApi};
#[allow(unused_imports)]
use log::{debug, error, info, log_enabled, trace, Level};
use std::collections::HashMap;
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
        conf.http_enabled = cfg!(feature = "test-esplora");

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

        let input: Vec<_> = meta_tx
            .input
            .into_iter()
            .map(|x| x.into_raw_tx_input())
            .collect();

        if self.get_balance(None, None).unwrap() < Amount::from_sat(required_balance) {
            panic!("Insufficient funds in bitcoind. Please generate a few blocks with: `bitcoin-cli generatetoaddress 10 {}`", self.get_new_address(None, None).unwrap());
        }

        // FIXME: core can't create a tx with two outputs to the same address
        let tx = self
            .create_raw_transaction_hex(&input, &map, meta_tx.locktime, meta_tx.replaceable)
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
            .electrsd
            .client
            .transaction_broadcast(&deserialize(&tx.hex).unwrap())
            .unwrap();
        debug!("broadcasted to electrum {}", txid);

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

    pub fn generate(&mut self, num_blocks: u64, address: Option<Address>) {
        let address = address.unwrap_or_else(|| self.get_new_address(None, None).unwrap());
        let hashes = self.generate_to_address(num_blocks, &address).unwrap();
        let best_hash = hashes.last().unwrap();
        let height = self.get_block_info(best_hash).unwrap().height;

        self.wait_for_block(height);

        debug!("Generated blocks to new height {}", height);
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

#[cfg(test)]
/// This test module is intended to be used externally from bdk library, if the user
/// wants to test their custom [`Blockchain`] implementation.
///
/// Test functions can be used using the `make_blockchain_tests` macro as below
/// ```
/// use bdk::make_blockchain_tests;
///
/// pub fn init_blockchain(test_client: &TestClient) -> <YourBlockchain> {
///     .. // Define your blockchain from a `TestClient`
/// }
///
///     make_blockchain_tests![
///             init_blockchain,
///             tests(
///                 test_sync_simple,
///                 test_sync_stop_gap_20,
///                 test_sync_before_and_after_receive,
///                 test_sync_multiple_outputs_same_tx,
///                 ... // List of test case names you wanna run
///                 )
///     ]
/// }
/// ```
///
/// This will replicate the bdk::blockchain test cases for other custom Blockchain backends.
pub mod test {
    use super::*;
    use crate::bitcoin::{Network, Transaction};
    use crate::blockchain::Blockchain;
    use crate::database::MemoryDatabase;
    use crate::testutils::blockchain_tests::TestClient;
    use crate::types::KeychainKind;
    use crate::wallet::AddressIndex;
    use crate::{testutils, wallet};
    use crate::{FeeRate, SyncOptions, Wallet};

    fn get_wallet_from_descriptors(
        descriptors: &(String, Option<String>),
    ) -> Wallet<MemoryDatabase> {
        Wallet::new(
            &descriptors.0.to_string(),
            descriptors.1.as_ref(),
            Network::Regtest,
            MemoryDatabase::new(),
        )
        .unwrap()
    }

    #[allow(dead_code)]
    enum WalletType {
        WpkhSingleSig,
        TaprootKeySpend,
        TaprootScriptSpend,
        TaprootScriptSpend2,
        TaprootScriptSpend3,
    }

    fn init_wallet<B>(
        ty: WalletType,
        init_blockchain: &dyn Fn(&TestClient) -> B,
    ) -> (
        Wallet<MemoryDatabase>,
        B,
        (String, Option<String>),
        TestClient,
    )
    where
        B: Blockchain,
    {
        let _ = env_logger::try_init();

        let descriptors = match ty {
            WalletType::WpkhSingleSig => testutils! {
                @descriptors ( "wpkh(Alice)" ) ( "wpkh(Alice)" ) ( @keys ( "Alice" => (@generate_xprv "/44'/0'/0'/0/*", "/44'/0'/0'/1/*") ) )
            },
            WalletType::TaprootKeySpend => testutils! {
                @descriptors ( "tr(Alice)" ) ( "tr(Alice)" ) ( @keys ( "Alice" => (@generate_xprv "/44'/0'/0'/0/*", "/44'/0'/0'/1/*") ) )
            },
            WalletType::TaprootScriptSpend => testutils! {
                @descriptors ( "tr(Key,and_v(v:pk(Script),older(6)))" ) ( "tr(Key,and_v(v:pk(Script),older(6)))" ) ( @keys ( "Key" => (@literal "30e14486f993d5a2d222770e97286c56cec5af115e1fb2e0065f476a0fcf8788"), "Script" => (@generate_xprv "/0/*", "/1/*") ) )
            },
            WalletType::TaprootScriptSpend2 => testutils! {
                @descriptors ( "tr(Alice,pk(Bob))" ) ( "tr(Alice,pk(Bob))" ) ( @keys ( "Alice" => (@literal "30e14486f993d5a2d222770e97286c56cec5af115e1fb2e0065f476a0fcf8788"), "Bob" => (@generate_xprv "/0/*", "/1/*") ) )
            },
            WalletType::TaprootScriptSpend3 => testutils! {
                @descriptors ( "tr(Alice,{pk(Bob),pk(Carol)})" ) ( "tr(Alice,{pk(Bob),pk(Carol)})" ) ( @keys ( "Alice" => (@literal "30e14486f993d5a2d222770e97286c56cec5af115e1fb2e0065f476a0fcf8788"), "Bob" => (@generate_xprv "/0/*", "/1/*"), "Carol" => (@generate_xprv "/0/*", "/1/*") ) )
            },
        };

        let test_client = TestClient::default();
        let blockchain = init_blockchain(&test_client);
        let wallet = get_wallet_from_descriptors(&descriptors);

        // rpc need to call import_multi before receiving any tx, otherwise will not see tx in the mempool
        #[cfg(any(feature = "test-rpc", feature = "test-rpc-legacy"))]
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        (wallet, blockchain, descriptors, test_client)
    }

    fn init_single_sig<B>(
        init_blockchain: &dyn Fn(&TestClient) -> B,
    ) -> (
        Wallet<MemoryDatabase>,
        B,
        (String, Option<String>),
        TestClient,
    )
    where
        B: Blockchain,
    {
        init_wallet(WalletType::WpkhSingleSig, init_blockchain)
    }

    pub fn test_sync_simple<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        use crate::database::Database;

        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        let tx = testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        };
        println!("{:?}", tx);
        let txid = test_client.receive(tx);

        // the RPC blockchain needs to call `sync()` during initialization to import the
        // addresses (see `init_single_sig()`), so we skip this assertion
        #[cfg(not(any(feature = "test-rpc", feature = "test-rpc-legacy")))]
        assert!(
            wallet.database().deref().get_sync_time().unwrap().is_none(),
            "initial sync_time not none"
        );

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert!(
            wallet.database().deref().get_sync_time().unwrap().is_some(),
            "sync_time hasn't been updated"
        );

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_unspent().unwrap()[0].keychain,
            KeychainKind::External,
            "incorrect keychain kind"
        );

        let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
        assert_eq!(list_tx_item.txid, txid, "incorrect txid");
        assert_eq!(list_tx_item.received, 50_000, "incorrect received");
        assert_eq!(list_tx_item.sent, 0, "incorrect sent");
        assert_eq!(
            list_tx_item.confirmation_time, None,
            "incorrect confirmation time"
        );
    }

    pub fn test_sync_stop_gap_20<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 5) => 50_000 )
        });
        test_client.receive(testutils! {
            @tx ( (@external descriptors, 25) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            100_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            2,
            "incorrect number of txs"
        );
    }

    pub fn test_sync_before_and_after_receive<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(wallet.get_balance().unwrap().get_total(), 0);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            1,
            "incorrect number of txs"
        );
    }

    pub fn test_sync_multiple_outputs_same_tx<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        let txid = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000, (@external descriptors, 5) => 30_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            105_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            1,
            "incorrect number of txs"
        );
        assert_eq!(
            wallet.list_unspent().unwrap().len(),
            3,
            "incorrect number of unspents"
        );

        let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
        assert_eq!(list_tx_item.txid, txid, "incorrect txid");
        assert_eq!(list_tx_item.received, 105_000, "incorrect received");
        assert_eq!(list_tx_item.sent, 0, "incorrect sent");
        assert_eq!(
            list_tx_item.confirmation_time, None,
            "incorrect confirmation_time"
        );
    }

    pub fn test_sync_receive_multi<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });
        test_client.receive(testutils! {
            @tx ( (@external descriptors, 5) => 25_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            75_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            2,
            "incorrect number of txs"
        );
        assert_eq!(
            wallet.list_unspent().unwrap().len(),
            2,
            "incorrect number of unspent"
        );
    }

    pub fn test_sync_address_reuse<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(wallet.get_balance().unwrap().untrusted_pending, 50_000);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 25_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            75_000,
            "incorrect balance"
        );
    }

    pub fn test_sync_receive_rbf_replaced<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        let txid = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 ) ( @replaceable true )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            1,
            "incorrect number of txs"
        );
        assert_eq!(wallet.list_unspent().unwrap().len(), 1, "incorrect unspent");

        let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
        assert_eq!(list_tx_item.txid, txid, "incorrect txid");
        assert_eq!(list_tx_item.received, 50_000, "incorrect received");
        assert_eq!(list_tx_item.sent, 0, "incorrect sent");
        assert_eq!(
            list_tx_item.confirmation_time, None,
            "incorrect confirmation_time"
        );

        let new_txid = test_client.bump_fee(&txid);

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance after bump"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            1,
            "incorrect number of txs after bump"
        );
        assert_eq!(
            wallet.list_unspent().unwrap().len(),
            1,
            "incorrect unspent after bump"
        );

        let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
        assert_eq!(list_tx_item.txid, new_txid, "incorrect txid after bump");
        assert_eq!(
            list_tx_item.received, 50_000,
            "incorrect received after bump"
        );
        assert_eq!(list_tx_item.sent, 0, "incorrect sent after bump");
        assert_eq!(
            list_tx_item.confirmation_time, None,
            "incorrect height after bump"
        );
    }

    // FIXME: I would like this to be cfg_attr(not(feature = "test-esplora"), ignore) but it
    // doesn't work for some reason.
    #[cfg(not(feature = "test-esplora"))]
    pub fn test_sync_reorg_block<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        let txid = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 ) ( @confirmations 1 ) ( @replaceable true )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000,
            "incorrect balance"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            1,
            "incorrect number of txs"
        );
        assert_eq!(
            wallet.list_unspent().unwrap().len(),
            1,
            "incorrect number of unspents"
        );

        let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
        assert_eq!(list_tx_item.txid, txid, "incorrect txid");
        assert!(
            list_tx_item.confirmation_time.is_some(),
            "incorrect confirmation_time"
        );

        // Invalidate 1 block
        test_client.invalidate(1);

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance after invalidate"
        );

        let list_tx_item = &wallet.list_transactions(false).unwrap()[0];
        assert_eq!(list_tx_item.txid, txid, "incorrect txid after invalidate");
        assert_eq!(
            list_tx_item.confirmation_time, None,
            "incorrect confirmation time after invalidate"
        );
    }

    pub fn test_sync_after_send<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        println!("{}", descriptors.0);
        let node_addr = test_client.get_node_address(None);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        builder.add_recipient(node_addr.script_pubkey(), 25_000);
        let (mut psbt, details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        let tx = psbt.extract_tx();
        println!("{}", bitcoin::consensus::encode::serialize_hex(&tx));
        blockchain.broadcast(&tx).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().trusted_pending,
            details.received,
            "incorrect balance after send"
        );

        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            2,
            "incorrect number of txs"
        );
        assert_eq!(
            wallet.list_unspent().unwrap().len(),
            1,
            "incorrect number of unspents"
        );
    }

    // Syncing wallet should not result in wallet address index to decrement.
    // This is critical as we should always ensure to not reuse addresses.
    pub fn test_sync_address_index_should_not_decrement<B>(
        init_blockchain: &dyn Fn(&TestClient) -> B,
    ) where
        B: Blockchain,
    {
        let (wallet, blockchain, _descriptors, mut test_client) = init_single_sig(init_blockchain);

        const ADDRS_TO_FUND: u32 = 7;
        const ADDRS_TO_IGNORE: u32 = 11;

        let mut first_addr_index: u32 = 0;

        (0..ADDRS_TO_FUND + ADDRS_TO_IGNORE).for_each(|i| {
            let new_addr = wallet.get_address(AddressIndex::New).unwrap();

            if i == 0 {
                first_addr_index = new_addr.index;
            }
            assert_eq!(
                new_addr.index,
                i + first_addr_index,
                "unexpected new address index (before sync)"
            );

            if i < ADDRS_TO_FUND {
                test_client.receive(testutils! {
                    @tx ((@addr new_addr.address) => 50_000)
                });
            }
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        let new_addr = wallet.get_address(AddressIndex::New).unwrap();
        assert_eq!(
            new_addr.index,
            ADDRS_TO_FUND + ADDRS_TO_IGNORE + first_addr_index,
            "unexpected new address index (after sync)"
        );
    }

    // Even if user does not explicitly grab new addresses, the address index should
    // increment after sync (if wallet has a balance).
    pub fn test_sync_address_index_should_increment<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        const START_FUND: u32 = 4;
        const END_FUND: u32 = 20;

        // "secretly" fund wallet via given range
        (START_FUND..END_FUND).for_each(|addr_index| {
            test_client.receive(testutils! {
                @tx ((@external descriptors, addr_index) => 50_000)
            });
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        let address = wallet.get_address(AddressIndex::New).unwrap();
        assert_eq!(
            address.index, END_FUND,
            "unexpected new address index (after sync)"
        );
    }

    /// Send two conflicting transactions to the same address twice in a row.
    /// The coins should only be received once!
    pub fn test_sync_double_receive<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let receiver_wallet = get_wallet_from_descriptors(&(
            "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)".to_string(),
            None,
        ));
        // need to sync so rpc can start watching
        receiver_wallet
            .sync(&blockchain, SyncOptions::default())
            .unwrap();

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
        });

        wallet
            .sync(&blockchain, SyncOptions::default())
            .expect("sync");
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            75_000,
            "incorrect balance"
        );
        let target_addr = receiver_wallet
            .get_address(wallet::AddressIndex::New)
            .unwrap()
            .address;

        let tx1 = {
            let mut builder = wallet.build_tx();
            builder
                .add_recipient(target_addr.script_pubkey(), 49_000)
                .enable_rbf();
            let (mut psbt, _details) = builder.finish().expect("building first tx");
            let finalized = wallet
                .sign(&mut psbt, Default::default())
                .expect("signing first tx");
            assert!(finalized, "Cannot finalize transaction");
            psbt.extract_tx()
        };

        let tx2 = {
            let mut builder = wallet.build_tx();
            builder
                .add_recipient(target_addr.script_pubkey(), 49_000)
                .enable_rbf()
                .fee_rate(FeeRate::from_sat_per_vb(5.0));
            let (mut psbt, _details) = builder.finish().expect("building replacement tx");
            let finalized = wallet
                .sign(&mut psbt, Default::default())
                .expect("signing replacement tx");
            assert!(finalized, "Cannot finalize transaction");
            psbt.extract_tx()
        };

        blockchain.broadcast(&tx1).expect("broadcasting first");
        blockchain
            .broadcast(&tx2)
            .expect("broadcasting replacement");

        receiver_wallet
            .sync(&blockchain, SyncOptions::default())
            .expect("syncing receiver");
        assert_eq!(
            receiver_wallet.get_balance().unwrap().untrusted_pending,
            49_000,
            "should have received coins once and only once"
        );
    }

    pub fn test_sync_many_sends_to_a_single_address<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        for _ in 0..4 {
            // split this up into multiple blocks so rpc doesn't get angry
            for _ in 0..20 {
                test_client.receive(testutils! {
                    @tx ( (@external descriptors, 0) => 1_000 )
                });
            }
            test_client.generate(1, None);
        }

        // add some to the mempool as well.
        for _ in 0..20 {
            test_client.receive(testutils! {
                @tx ( (@external descriptors, 0) => 1_000 )
            });
        }

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        let balance = wallet.get_balance().unwrap();
        assert_eq!(balance.untrusted_pending + balance.get_spendable(), 100_000);
    }

    pub fn test_update_confirmation_time_after_generate<B>(
        init_blockchain: &dyn Fn(&TestClient) -> B,
    ) where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        println!("{}", descriptors.0);
        let node_addr = test_client.get_node_address(None);

        let received_txid = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );

        let tx_map = wallet
            .list_transactions(false)
            .unwrap()
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect::<std::collections::HashMap<_, _>>();
        let details = tx_map.get(&received_txid).unwrap();
        assert!(details.confirmation_time.is_none());

        test_client.generate(1, Some(node_addr));
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        let tx_map = wallet
            .list_transactions(false)
            .unwrap()
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect::<std::collections::HashMap<_, _>>();
        let details = tx_map.get(&received_txid).unwrap();
        assert!(details.confirmation_time.is_some());
    }

    pub fn test_sync_outgoing_from_scratch<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);
        let received_txid = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        builder.add_recipient(node_addr.script_pubkey(), 25_000);
        let (mut psbt, details) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        let sent_tx = psbt.extract_tx();
        blockchain.broadcast(&sent_tx).unwrap();

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            details.received,
            "incorrect balance after receive"
        );

        // empty wallet
        let wallet = get_wallet_from_descriptors(&descriptors);

        #[cfg(feature = "rpc")] // rpc cannot see mempool tx before importmulti
        test_client.generate(1, Some(node_addr));

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        let tx_map = wallet
            .list_transactions(false)
            .unwrap()
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect::<std::collections::HashMap<_, _>>();

        let received = tx_map.get(&received_txid).unwrap();
        assert_eq!(
            received.received, 50_000,
            "incorrect received from receiver"
        );
        assert_eq!(received.sent, 0, "incorrect sent from receiver");

        let sent = tx_map.get(&sent_tx.txid()).unwrap();
        assert_eq!(
            sent.received, details.received,
            "incorrect received from sender"
        );
        assert_eq!(sent.sent, details.sent, "incorrect sent from sender");
        assert_eq!(
            sent.fee.unwrap_or(0),
            details.fee.unwrap_or(0),
            "incorrect fees from sender"
        );
    }

    pub fn test_sync_long_change_chain<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );

        let mut total_sent = 0;
        for _ in 0..5 {
            let mut builder = wallet.build_tx();
            builder.add_recipient(node_addr.script_pubkey(), 5_000);
            let (mut psbt, details) = builder.finish().unwrap();
            let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
            assert!(finalized, "Cannot finalize transaction");
            blockchain.broadcast(&psbt.extract_tx()).unwrap();

            wallet.sync(&blockchain, SyncOptions::default()).unwrap();

            total_sent += 5_000 + details.fee.unwrap_or(0);
        }

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000 - total_sent,
            "incorrect balance after chain"
        );

        // empty wallet

        let wallet = get_wallet_from_descriptors(&descriptors);

        #[cfg(feature = "rpc")] // rpc cannot see mempool tx before importmulti
        test_client.generate(1, Some(node_addr));

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000 - total_sent,
            "incorrect balance empty wallet"
        );
    }

    pub fn test_sync_bump_fee_basic<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        builder
            .add_recipient(node_addr.script_pubkey().clone(), 5_000)
            .enable_rbf();
        let (mut psbt, details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000 - details.fee.unwrap_or(0) - 5_000,
            "incorrect balance from fees"
        );
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            details.received,
            "incorrect balance from received"
        );

        let mut builder = wallet.build_fee_bump(details.txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(2.1));
        let (mut new_psbt, new_details) = builder.finish().expect("fee bump tx");
        let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&new_psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000 - new_details.fee.unwrap_or(0) - 5_000,
            "incorrect balance from fees after bump"
        );
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            new_details.received,
            "incorrect balance from received after bump"
        );

        assert!(
            new_details.fee.unwrap_or(0) > details.fee.unwrap_or(0),
            "incorrect fees"
        );
    }

    pub fn test_sync_bump_fee_remove_change<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        builder
            .add_recipient(node_addr.script_pubkey().clone(), 49_000)
            .enable_rbf();
        let (mut psbt, details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            1_000 - details.fee.unwrap_or(0),
            "incorrect balance after send"
        );
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            details.received,
            "incorrect received after send"
        );

        let mut builder = wallet.build_fee_bump(details.txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(5.1));
        let (mut new_psbt, new_details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&new_psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            0,
            "incorrect balance after change removal"
        );
        assert_eq!(
            new_details.received, 0,
            "incorrect received after change removal"
        );

        assert!(
            new_details.fee.unwrap_or(0) > details.fee.unwrap_or(0),
            "incorrect fees"
        );
    }

    pub fn test_sync_bump_fee_add_input_simple<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            75_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        builder
            .add_recipient(node_addr.script_pubkey().clone(), 49_000)
            .enable_rbf();
        let (mut psbt, details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            26_000 - details.fee.unwrap_or(0),
            "incorrect balance after send"
        );
        assert_eq!(
            details.received,
            1_000 - details.fee.unwrap_or(0),
            "incorrect received after send"
        );

        let mut builder = wallet.build_fee_bump(details.txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(10.0));
        let (mut new_psbt, new_details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&new_psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(new_details.sent, 75_000, "incorrect sent");
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            new_details.received,
            "incorrect balance after add input"
        );
    }

    pub fn test_sync_bump_fee_add_input_no_change<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000, (@external descriptors, 1) => 25_000 ) (@confirmations 1)
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            75_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        builder
            .add_recipient(node_addr.script_pubkey().clone(), 49_000)
            .enable_rbf();
        let (mut psbt, details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            26_000 - details.fee.unwrap_or(0),
            "incorrect balance after send"
        );
        assert_eq!(
            details.received,
            1_000 - details.fee.unwrap_or(0),
            "incorrect received after send"
        );

        let mut builder = wallet.build_fee_bump(details.txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(123.0));
        let (mut new_psbt, new_details) = builder.finish().unwrap();
        println!("{:#?}", new_details);

        let finalized = wallet.sign(&mut new_psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        blockchain.broadcast(&new_psbt.extract_tx()).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(new_details.sent, 75_000, "incorrect sent");
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            0,
            "incorrect balance after add input"
        );
        assert_eq!(
            new_details.received, 0,
            "incorrect received after add input"
        );
    }

    pub fn test_add_data<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);
        let _ = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "incorrect balance"
        );

        let mut builder = wallet.build_tx();
        let data = [42u8; 80];
        builder.add_data(&data);
        let (mut psbt, details) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        let tx = psbt.extract_tx();
        let serialized_tx = bitcoin::consensus::encode::serialize(&tx);
        assert!(
            serialized_tx.windows(data.len()).any(|e| e == data),
            "cannot find op_return data in transaction"
        );
        blockchain.broadcast(&tx).unwrap();
        test_client.generate(1, Some(node_addr));
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            50_000 - details.fee.unwrap_or(0),
            "incorrect balance after send"
        );

        let tx_map = wallet
            .list_transactions(false)
            .unwrap()
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect::<std::collections::HashMap<_, _>>();
        let _ = tx_map.get(&tx.txid()).unwrap();
    }

    pub fn test_sync_receive_coinbase<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, _, mut test_client) = init_single_sig(init_blockchain);

        let wallet_addr = wallet
            .get_address(wallet::AddressIndex::New)
            .unwrap()
            .address;

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().immature,
            0,
            "incorrect balance"
        );

        test_client.generate(1, Some(wallet_addr));

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert!(
            wallet.get_balance().unwrap().immature > 0,
            "incorrect balance after receiving coinbase"
        );

        // make coinbase mature (100 blocks)
        let node_addr = test_client.get_node_address(None);
        test_client.generate(100, Some(node_addr));
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert!(
            wallet.get_balance().unwrap().confirmed > 0,
            "incorrect balance after maturing coinbase"
        );
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    pub fn test_send_to_bech32m_addr<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        use bitcoincore_rpc::jsonrpc::serde_json::Value;
        use bitcoincore_rpc::{Auth, Client};
        use serde::Serialize;

        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);

        // TODO remove once rust-bitcoincore-rpc with PR 199 released
        // https://github.com/rust-bitcoin/rust-bitcoincore-rpc/pull/199
        /// Import Descriptor Request
        #[derive(Serialize, Clone, PartialEq, Eq, Debug)]
        pub struct ImportDescriptorRequest {
            pub active: bool,
            #[serde(rename = "desc")]
            pub descriptor: String,
            pub range: [i64; 2],
            pub next_index: i64,
            pub timestamp: String,
            pub internal: bool,
        }

        // TODO remove once rust-bitcoincore-rpc with PR 199 released
        impl ImportDescriptorRequest {
            /// Create a new Import Descriptor request providing just the descriptor and internal flags
            pub fn new(descriptor: &str, internal: bool) -> Self {
                ImportDescriptorRequest {
                    descriptor: descriptor.to_string(),
                    internal,
                    active: true,
                    range: [0, 100],
                    next_index: 0,
                    timestamp: "now".to_string(),
                }
            }
        }

        // 1. Create and add descriptors to a test bitcoind node taproot wallet

        // TODO replace once rust-bitcoincore-rpc with PR 174 released
        // https://github.com/rust-bitcoin/rust-bitcoincore-rpc/pull/174
        let _createwallet_result: Value = test_client
            .bitcoind
            .client
            .call(
                "createwallet",
                &[
                    "taproot_wallet".into(),
                    false.into(),
                    true.into(),
                    serde_json::to_value("").unwrap(),
                    false.into(),
                    true.into(),
                ],
            )
            .unwrap();

        // TODO replace once bitcoind released with support for rust-bitcoincore-rpc PR 174
        let taproot_wallet_client = Client::new(
            &test_client.bitcoind.rpc_url_with_wallet("taproot_wallet"),
            Auth::CookieFile(test_client.bitcoind.params.cookie_file.clone()),
        )
        .unwrap();

        let wallet_descriptor = "tr(tprv8ZgxMBicQKsPdBtxmEMPnNq58KGusNAimQirKFHqX2yk2D8q1v6pNLiKYVAdzDHy2w3vF4chuGfMvNtzsbTTLVXBcdkCA1rje1JG6oksWv8/86h/1h/0h/0/*)#y283ssmn";
        let change_descriptor = "tr(tprv8ZgxMBicQKsPdBtxmEMPnNq58KGusNAimQirKFHqX2yk2D8q1v6pNLiKYVAdzDHy2w3vF4chuGfMvNtzsbTTLVXBcdkCA1rje1JG6oksWv8/86h/1h/0h/1/*)#47zsd9tt";

        let tr_descriptors = vec![
            ImportDescriptorRequest::new(wallet_descriptor, false),
            ImportDescriptorRequest::new(change_descriptor, false),
        ];

        // TODO replace once rust-bitcoincore-rpc with PR 199 released
        let _import_result: Value = taproot_wallet_client
            .call(
                "importdescriptors",
                &[serde_json::to_value(tr_descriptors).unwrap()],
            )
            .unwrap();

        // 2. Get a new bech32m address from test bitcoind node taproot wallet

        // TODO replace once rust-bitcoincore-rpc with PR 199 released
        let node_addr: bitcoin::Address = taproot_wallet_client
            .call("getnewaddress", &["test address".into(), "bech32m".into()])
            .unwrap();
        assert_eq!(
            node_addr,
            bitcoin::Address::from_str(
                "bcrt1pj5y3f0fu4y7g98k4v63j9n0xvj3lmln0cpwhsjzknm6nt0hr0q7qnzwsy9"
            )
            .unwrap()
        );

        // 3. Send 50_000 sats from test bitcoind node to test BDK wallet

        test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().untrusted_pending,
            50_000,
            "wallet has incorrect balance"
        );

        // 4. Send 25_000 sats from test BDK wallet to test bitcoind node taproot wallet

        let mut builder = wallet.build_tx();
        builder.add_recipient(node_addr.script_pubkey(), 25_000);
        let (mut psbt, details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "wallet cannot finalize transaction");
        let tx = psbt.extract_tx();
        blockchain.broadcast(&tx).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(
            wallet.get_balance().unwrap().get_spendable(),
            details.received,
            "wallet has incorrect balance after send"
        );
        assert_eq!(
            wallet.list_transactions(false).unwrap().len(),
            2,
            "wallet has incorrect number of txs"
        );
        assert_eq!(
            wallet.list_unspent().unwrap().len(),
            1,
            "wallet has incorrect number of unspents"
        );
        test_client.generate(1, None);

        // 5. Verify 25_000 sats are received by test bitcoind node taproot wallet

        let taproot_balance = taproot_wallet_client.get_balance(None, None).unwrap();
        assert_eq!(
            taproot_balance.as_sat(),
            25_000,
            "node has incorrect taproot wallet balance"
        );
    }

    pub fn test_tx_chain<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        // use $crate::wallet::AddressIndex;

        // Here we want to test that we set correctly the send and receive
        // fields in the transaction object. For doing so, we create two
        // different txs, the second one spending from the first:
        // 1.
        // Core (#1) -> Core (#2)
        //           -> Us   (#3)
        // 2.
        // Core (#2) -> Us   (#4)

        let (wallet, blockchain, _, mut test_client) = init_single_sig(init_blockchain);
        let bdk_address = wallet.get_address(AddressIndex::New).unwrap().address;
        let core_address = test_client.get_new_address(None, None).unwrap();
        let tx = testutils! {
            @tx ( (@addr bdk_address.clone()) => 50_000, (@addr core_address.clone()) => 40_000 )
        };

        // Tx one: from Core #1 to Core #2 and Us #3.
        let txid_1 = test_client.receive(tx);
        let tx_1: Transaction =
            deserialize(&test_client.get_transaction(&txid_1, None).unwrap().hex).unwrap();
        let vout_1 = tx_1
            .output
            .into_iter()
            .position(|o| o.script_pubkey == core_address.script_pubkey())
            .unwrap() as u32;
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        let tx_1 = wallet
            .list_transactions(false)
            .unwrap()
            .into_iter()
            .find(|tx| tx.txid == txid_1)
            .unwrap();
        assert_eq!(tx_1.received, 50_000);
        assert_eq!(tx_1.sent, 0);

        // Tx two: from Core #2 to Us #4.
        let tx = testutils! {
            @tx ( (@addr bdk_address) => 10_000 ) ( @inputs (txid_1,vout_1))
        };
        let txid_2 = test_client.receive(tx);

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        let tx_2 = wallet
            .list_transactions(false)
            .unwrap()
            .into_iter()
            .find(|tx| tx.txid == txid_2)
            .unwrap();
        assert_eq!(tx_2.received, 10_000);
        assert_eq!(tx_2.sent, 0);
    }

    pub fn test_double_spend<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        // We create a tx and then we try to double spend it; BDK will always allow
        // us to do so, as it never forgets about spent UTXOs
        let (wallet, blockchain, descriptors, mut test_client) = init_single_sig(init_blockchain);
        let node_addr = test_client.get_node_address(None);
        let _ = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(node_addr.script_pubkey(), 25_000);
        let (mut psbt, _details) = builder.finish().unwrap();
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized, "Cannot finalize transaction");
        let initial_tx = psbt.extract_tx();
        let _sent_txid = blockchain.broadcast(&initial_tx).unwrap();
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        for utxo in wallet.list_unspent().unwrap() {
            // Making sure the TXO we just spent is not returned by list_unspent
            assert!(
                utxo.outpoint != initial_tx.input[0].previous_output,
                "wallet displays spent txo in unspents"
            );
        }
        // We can still create a transaction double spending `initial_tx`
        let mut builder = wallet.build_tx();
        builder
            .add_utxo(initial_tx.input[0].previous_output)
            .expect("Can't manually add an UTXO spent");
        test_client.generate(1, Some(node_addr));
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        // Even after confirmation, we can still create a tx double spend it
        let mut builder = wallet.build_tx();
        builder
            .add_utxo(initial_tx.input[0].previous_output)
            .expect("Can't manually add an UTXO spent");
        for utxo in wallet.list_unspent().unwrap() {
            // Making sure the TXO we just spent is not returned by list_unspent
            assert!(
                utxo.outpoint != initial_tx.input[0].previous_output,
                "wallet displays spent txo in unspents"
            );
        }
    }

    pub fn test_send_receive_pkh<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let descriptors = (
            "pkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)".to_string(),
            None,
        );
        let mut test_client = TestClient::default();
        let blockchain = init_blockchain(&test_client);
        let wallet = get_wallet_from_descriptors(&descriptors);
        #[cfg(any(feature = "test-rpc", feature = "test-rpc-legacy"))]
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        let _ = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0)   => 50_000 )
        });

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();

        assert_eq!(wallet.get_balance().unwrap().untrusted_pending, 50_000);

        let tx = {
            let mut builder = wallet.build_tx();
            builder.add_recipient(test_client.get_node_address(None).script_pubkey(), 25_000);
            let (mut psbt, _details) = builder.finish().unwrap();
            wallet.sign(&mut psbt, Default::default()).unwrap();
            psbt.extract_tx()
        };
        blockchain.broadcast(&tx).unwrap();

        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    pub fn test_taproot_key_spend<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) =
            init_wallet(WalletType::TaprootKeySpend, init_blockchain);

        let _ = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 )
        });
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(wallet.get_balance().unwrap().untrusted_pending, 50_000);

        let tx = {
            let mut builder = wallet.build_tx();
            builder.add_recipient(test_client.get_node_address(None).script_pubkey(), 25_000);
            let (mut psbt, _details) = builder.finish().unwrap();
            wallet.sign(&mut psbt, Default::default()).unwrap();
            psbt.extract_tx()
        };
        blockchain.broadcast(&tx).unwrap();
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    pub fn test_taproot_script_spend<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        let (wallet, blockchain, descriptors, mut test_client) =
            init_wallet(WalletType::TaprootScriptSpend, init_blockchain);

        let _ = test_client.receive(testutils! {
            @tx ( (@external descriptors, 0) => 50_000 ) ( @confirmations 6 )
        });
        wallet.sync(&blockchain, SyncOptions::default()).unwrap();
        assert_eq!(wallet.get_balance().unwrap().get_spendable(), 50_000);

        let ext_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
        let int_policy = wallet.policies(KeychainKind::Internal).unwrap().unwrap();

        let ext_path = vec![(ext_policy.id.clone(), vec![1])].into_iter().collect();
        let int_path = vec![(int_policy.id.clone(), vec![1])].into_iter().collect();

        let tx = {
            let mut builder = wallet.build_tx();
            builder
                .add_recipient(test_client.get_node_address(None).script_pubkey(), 25_000)
                .policy_path(ext_path, KeychainKind::External)
                .policy_path(int_path, KeychainKind::Internal);
            let (mut psbt, _details) = builder.finish().unwrap();
            wallet.sign(&mut psbt, Default::default()).unwrap();
            psbt.extract_tx()
        };
        blockchain.broadcast(&tx).unwrap();
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    pub fn test_sign_taproot_core_keyspend_psbt<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        test_sign_taproot_core_psbt(WalletType::TaprootKeySpend, init_blockchain);
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    pub fn test_sign_taproot_core_scriptspend2_psbt<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        test_sign_taproot_core_psbt(WalletType::TaprootScriptSpend2, init_blockchain);
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    pub fn test_sign_taproot_core_scriptspend3_psbt<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        test_sign_taproot_core_psbt(WalletType::TaprootScriptSpend3, init_blockchain);
    }

    #[cfg(not(feature = "test-rpc-legacy"))]
    fn test_sign_taproot_core_psbt<B>(
        wallet_type: WalletType,
        init_blockchain: &dyn Fn(&TestClient) -> B,
    ) where
        B: Blockchain,
    {
        use bitcoincore_rpc::jsonrpc::serde_json::Value;
        use bitcoincore_rpc::{Auth, Client};

        let (wallet, _blockchain, _descriptors, test_client) =
            init_wallet(wallet_type, init_blockchain);

        // TODO replace once rust-bitcoincore-rpc with PR 174 released
        // https://github.com/rust-bitcoin/rust-bitcoincore-rpc/pull/174
        let _createwallet_result: Value = test_client
            .bitcoind
            .client
            .call(
                "createwallet",
                &[
                    "taproot_wallet".into(),
                    true.into(),
                    true.into(),
                    serde_json::to_value("").unwrap(),
                    false.into(),
                    true.into(),
                    true.into(),
                    false.into(),
                ],
            )
            .expect("created wallet");

        let external_descriptor = wallet.get_descriptor_for_keychain(KeychainKind::External);

        // TODO replace once bitcoind released with support for rust-bitcoincore-rpc PR 174
        let taproot_wallet_client = Client::new(
            &test_client.bitcoind.rpc_url_with_wallet("taproot_wallet"),
            Auth::CookieFile(test_client.bitcoind.params.cookie_file.clone()),
        )
        .unwrap();

        let descriptor_info = taproot_wallet_client
            .get_descriptor_info(external_descriptor.to_string().as_str())
            .expect("descriptor info");

        let import_descriptor_args = json!([{
            "desc": descriptor_info.descriptor,
            "active": true,
            "timestamp": "now",
            "label":"taproot key spend",
        }]);
        let _importdescriptors_result: Value = taproot_wallet_client
            .call("importdescriptors", &[import_descriptor_args])
            .expect("import wallet");
        let generate_to_address: bitcoin::Address = taproot_wallet_client
            .call("getnewaddress", &["test address".into(), "bech32m".into()])
            .expect("new address");
        let _generatetoaddress_result = taproot_wallet_client
            .generate_to_address(101, &generate_to_address)
            .expect("generated to address");
        let send_to_address = wallet
            .get_address(crate::wallet::AddressIndex::New)
            .unwrap()
            .address
            .to_string();
        let change_address = wallet
            .get_address(crate::wallet::AddressIndex::New)
            .unwrap()
            .address
            .to_string();
        let send_addr_amounts = json!([{
            send_to_address: "0.4321"
        }]);
        let send_options = json!({
            "change_address": change_address,
            "psbt": true,
        });
        let send_result: Value = taproot_wallet_client
            .call(
                "send",
                &[
                    send_addr_amounts,
                    Value::Null,
                    "unset".into(),
                    Value::Null,
                    send_options,
                ],
            )
            .expect("send psbt");
        let core_psbt = send_result["psbt"].as_str().expect("core psbt str");

        use bitcoin::util::psbt::PartiallySignedTransaction;

        // Test parsing core created PSBT
        let mut psbt = PartiallySignedTransaction::from_str(&core_psbt).expect("core taproot psbt");

        // Test signing core created PSBT
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert_eq!(finalized, true);

        // Test with updated psbt
        let update_result: Value = taproot_wallet_client
            .call("utxoupdatepsbt", &[core_psbt.into()])
            .expect("update psbt utxos");
        let core_updated_psbt = update_result.as_str().expect("core updated psbt");

        // Test parsing core created and updated PSBT
        let mut psbt =
            PartiallySignedTransaction::from_str(&core_updated_psbt).expect("core taproot psbt");

        // Test signing core created and updated PSBT
        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert_eq!(finalized, true);
    }

    pub fn test_get_block_hash<B>(init_blockchain: &dyn Fn(&TestClient) -> B)
    where
        B: Blockchain,
    {
        // create wallet with init_wallet
        let (_, blockchain, _descriptors, mut test_client) = init_single_sig(init_blockchain);

        let height = test_client
            .bitcoind
            .client
            .get_blockchain_info()
            .unwrap()
            .blocks as u64;
        let best_hash = test_client.bitcoind.client.get_best_block_hash().unwrap();

        // use get_block_hash to get best block hash and compare with best_hash above
        let block_hash = blockchain.get_block_hash(height).unwrap();
        assert_eq!(best_hash, block_hash);

        // generate blocks to address
        let node_addr = test_client.get_node_address(None);
        test_client.generate(10, Some(node_addr));

        let height = test_client
            .bitcoind
            .client
            .get_blockchain_info()
            .unwrap()
            .blocks as u64;
        let best_hash = test_client.bitcoind.client.get_best_block_hash().unwrap();

        let block_hash = blockchain.get_block_hash(height).unwrap();
        assert_eq!(best_hash, block_hash);

        // try to get hash for block that has not yet been created.
        assert!(blockchain.get_block_hash(height + 1).is_err());
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! make_blockchain_tests {
    ($fn_name:ident, tests ( $($x:tt) , + $(,)? )) => {
        $(
          #[test]
          fn $x()
          {
            $crate::testutils::blockchain_tests::test::$x(&$fn_name);
          }
        )+
    };
}
