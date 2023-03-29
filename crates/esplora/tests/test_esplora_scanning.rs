use anyhow::Result;
use bdk_chain::{
    bitcoin::{hashes::Hash, BlockHash, OutPoint, Transaction, Txid},
    bitcoin::{Address, Amount, Network::Regtest, Script},
    collections::BTreeMap,
    keychain::KeychainTxOutIndex,
    miniscript::{Descriptor, DescriptorPublicKey},
    sparse_chain::ChainPosition,
    TxHeight,
};
use bdk_esplora::{EsploraAsyncExt, EsploraExt};
use electrsd::{
    bitcoind::{
        self,
        bitcoincore_rpc::{bitcoincore_rpc_json::AddressType, RpcApi},
        BitcoinD,
    },
    electrum_client::ElectrumApi,
    ElectrsD,
};
use esplora_client::{AsyncClient, BlockingClient, Builder};
use std::{
    env,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialOrd, PartialEq, Ord, Eq)]
enum Keychain {
    External,
    Internal,
}

struct TestFramework {
    bitcoin_daemon: BitcoinD,
    electrs_daemon: ElectrsD,
    async_client: Option<AsyncClient>,
    blocking_client: Option<BlockingClient>,
}

impl TestFramework {
    pub fn init(
        is_async: bool,
        bitcoind_conf: Option<bitcoind::Conf>,
        electrsd_conf: Option<electrsd::Conf>,
    ) -> Self {
        let bitcoind_exe = env::var("BITCOIND_EXE")
            .ok()
            .or_else(|| bitcoind::downloaded_exe_path().ok())
            .expect(
                "you need to provide an env var BITCOIND_EXE or specify a bitcoind version feature",
            );
        let bitcoind_conf = bitcoind_conf.unwrap_or_default();

        let bitcoin_daemon = BitcoinD::with_conf(bitcoind_exe, &bitcoind_conf).unwrap();

        let electrs_exe = env::var("ELECTRS_EXE")
            .ok()
            .or_else(electrsd::downloaded_exe_path)
            .expect(
                "you need to provide env var ELECTRS_EXE or specify an electrsd version feature",
            );
        let mut electrsd_conf = electrsd_conf.unwrap_or_default();
        electrsd_conf.http_enabled = true;

        let electrs_daemon =
            ElectrsD::with_conf(electrs_exe, &bitcoin_daemon, &electrsd_conf).unwrap();

        let esplora_url = format!("http://{}", electrs_daemon.esplora_url.as_ref().unwrap());
        let client_builder = Builder::new(&esplora_url);

        if is_async {
            let async_client = AsyncClient::from_builder(client_builder)
                .expect("creation of Rust Esplora Client failed");
            Self {
                bitcoin_daemon,
                electrs_daemon,
                async_client: Some(async_client),
                blocking_client: None,
            }
        } else {
            let blocking_client = BlockingClient::from_builder(client_builder)
                .expect("creation of Rust Esplora Client failed");
            Self {
                bitcoin_daemon,
                electrs_daemon,
                async_client: None,
                blocking_client: Some(blocking_client),
            }
        }
    }

    fn wait_for_block(&self, min_height: usize) {
        let mut header = self
            .electrs_daemon
            .client
            .block_headers_subscribe()
            .unwrap();
        loop {
            if header.height >= min_height {
                break;
            }
            header = Self::exponential_backoff_poll(|| {
                self.electrs_daemon.trigger().unwrap();
                self.electrs_daemon.client.ping().unwrap();
                self.electrs_daemon.client.block_headers_pop().unwrap()
            });
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

    pub fn wait_for_tx_appears_in_esplora(
        &self,
        wait_seconds: u64,
        txid: &bdk_chain::bitcoin::Txid,
    ) -> bool {
        let instant = Instant::now();
        loop {
            let wait_tx = self.electrs_daemon.client.transaction_get(txid);
            if wait_tx.is_ok() {
                return true;
            }

            if instant.elapsed() >= Duration::from_secs(wait_seconds) {
                return false;
            }
        }
    }

    pub fn generate_blocks(&self, num: usize) {
        let address = self
            .bitcoin_daemon
            .client
            .get_new_address(Some("test"), Some(AddressType::Bech32))
            .unwrap();
        let _block_hashes = self
            .bitcoin_daemon
            .client
            .generate_to_address(num as u64, &address)
            .unwrap();
    }

    pub fn premine(&self, num_blocks: usize) {
        self.generate_blocks_and_wait(num_blocks);
    }

    pub fn generate_blocks_and_wait(&self, num: usize) {
        let curr_height = self.bitcoin_daemon.client.get_block_count().unwrap();
        self.generate_blocks(num);
        self.wait_for_block(curr_height as usize + num);
    }

    pub fn reorg(num_blocks: usize, bitcoin_daemon: &BitcoinD) -> Result<()> {
        let best_hash = bitcoin_daemon.client.get_best_block_hash()?;
        let initial_height = bitcoin_daemon.client.get_block_info(&best_hash)?.height;

        let mut to_invalidate = best_hash;
        for i in 1..=num_blocks {
            dbg!(
                "Invalidating block {}/{} ({})",
                i,
                num_blocks,
                to_invalidate
            );

            bitcoin_daemon.client.invalidate_block(&to_invalidate)?;
            to_invalidate = bitcoin_daemon.client.get_best_block_hash()?;
        }

        dbg!(
            "Invalidated {} blocks to new height of {}",
            num_blocks,
            initial_height - num_blocks
        );

        Ok(())
    }
}

#[test]
fn test_scanning_stop_gap() {
    let test_framework = TestFramework::init(false, None, None);
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 19);
    let (_, to_script) = revealed_spks.last().unwrap().to_owned();

    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, revealed_spks.into_iter());
    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .unwrap();

    assert_eq!(
        *keychain_scan
            .last_active_indices
            .get(&Keychain::External)
            .unwrap(),
        19
    );
    let (&conf, chain_tx) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert_eq!(tx, chain_tx.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(103));
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.script_pubkey == to_script)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        });
    assert_eq!(full_txout.unwrap().txout.value, 10000);

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 38);
    let (_, to_script) = revealed_spks.last().unwrap().to_owned();

    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, revealed_spks.into_iter());
    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .unwrap();

    assert_eq!(
        *keychain_scan
            .last_active_indices
            .get(&Keychain::External)
            .unwrap(),
        38
    );

    let (&conf, chain_tx) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert_eq!(tx, chain_tx.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(104));
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.script_pubkey == to_script)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        });
    assert_eq!(full_txout.unwrap().txout.value, 10000);

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 59);
    let (_, to_script) = revealed_spks.last().unwrap().to_owned();

    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, revealed_spks.into_iter());
    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .unwrap();

    assert!(keychain_scan
        .last_active_indices
        .get(&Keychain::External)
        .is_none());
    assert!(keychain_scan.update.get_tx_in_chain(tx.txid()).is_none());
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.script_pubkey == to_script)
        .unwrap();
    assert!(keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32
        })
        .is_none());
}

#[test]
fn test_reorg() -> Result<()> {
    let mut conf = bitcoind::Conf::default();
    conf.p2p = bitcoind::P2P::Yes;
    let test_framework = TestFramework::init(false, Some(conf), None);
    let mut miner_conf = bitcoind::Conf::default();
    miner_conf.p2p = test_framework.bitcoin_daemon.p2p_connect(true).unwrap();
    let bitcoind_exe = env::var("BITCOIND_EXE")
        .ok()
        .or_else(|| bitcoind::downloaded_exe_path().ok())
        .expect(
            "you need to provide an env var BITCOIND_EXE or specify a bitcoind version feature",
        );

    let miner_node = BitcoinD::with_conf(bitcoind_exe, &miner_conf).unwrap();
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 0);
    let (_, revealed_spk) = revealed_spks.last().unwrap().to_owned();

    // Get the transaction above confirmed
    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, [(0, revealed_spk.clone())].into_iter());
    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .unwrap();
    assert_eq!(
        *keychain_scan
            .last_active_indices
            .get(&Keychain::External)
            .unwrap(),
        0
    );
    let (&conf, chain_tx) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert_eq!(tx, chain_tx.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(103));
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.value == 10000)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        });
    assert_eq!(full_txout.unwrap().txout.value, 10000);

    // Reorg blocks on miner chain
    TestFramework::reorg(3, &miner_node)?;
    // Generate more blocks on the miner node, thereby making it the chain with the most
    // work, so the bitcoin_daemon chain has to catch up on this chain which doesn't
    // have a transaction above.
    let curr_height = miner_node.client.get_block_count().unwrap();
    let address = miner_node
        .client
        .get_new_address(Some("test"), Some(AddressType::Bech32))
        .unwrap();
    let _block_hashes = miner_node
        .client
        .generate_to_address(5u64, &address)
        .unwrap();
    test_framework.wait_for_block(5usize + curr_height as usize);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, [(0, revealed_spk)].into_iter());
    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .unwrap();
    let (conf, _tx_chain) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert!(!conf.is_confirmed());
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.value == 10000)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        })
        .unwrap();
    assert!(!full_txout.chain_position.is_confirmed());

    Ok(())
}

#[test]
fn test_scan_with_txids() {
    let test_framework = TestFramework::init(false, None, None);
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let (tx_1, _) = send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 0);

    test_framework.generate_blocks_and_wait(1);

    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [tx_1.txid()],
            [],
            20,
            5,
        )
        .unwrap();
    assert!(keychain_scan
        .last_active_indices
        .get(&Keychain::External)
        .is_none());
    let (conf, tx1_chain) = keychain_scan.update.get_tx_in_chain(tx_1.txid()).unwrap();
    assert_eq!(tx_1, tx1_chain.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(103));
    let checkpoint = keychain_scan.update.chain().checkpoint_at(103).unwrap();
    assert_eq!(
        checkpoint.hash,
        test_framework
            .bitcoin_daemon
            .client
            .get_block_hash(103)
            .unwrap()
    );

    let (tx_2, _) = send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 1);

    test_framework.wait_for_tx_appears_in_esplora(5, &tx_2.txid());
    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [tx_2.txid()],
            [],
            20,
            5,
        )
        .unwrap();
    let (conf, _tx_chain) = keychain_scan.update.get_tx_in_chain(tx_2.txid()).unwrap();
    assert!(!conf.is_confirmed());
}

#[test]
fn test_scan_with_outpoints() {
    let test_framework = TestFramework::init(false, None, None);
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let mut outpoints: [OutPoint; 2] = [OutPoint::null(), OutPoint::null()];
    let mut txids: [Txid; 2] = [Txid::from_inner([0x00; 32]), Txid::from_inner([0x00; 32])];
    for i in 0..=1 {
        let (tx, revealed_spks) =
            send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, i);
        let (_, revealed_spk) = revealed_spks.get(0).unwrap().to_owned();
        let (output_vout, _) = tx
            .output
            .iter()
            .enumerate()
            .find(|(_idx, out)| out.script_pubkey == revealed_spk.clone())
            .unwrap();
        outpoints[i as usize] = OutPoint::new(tx.txid(), output_vout as u32);
        txids[i as usize] = tx.txid();
    }

    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [],
            outpoints,
            20,
            5,
        )
        .unwrap();
    for i in 0..=1 {
        assert!(keychain_scan.update.get_tx_in_chain(txids[i]).is_none());
        assert!(keychain_scan.update.full_txout(outpoints[i]).is_none());
    }

    test_framework.generate_blocks_and_wait(1);

    let keychain_scan = test_framework
        .blocking_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [],
            outpoints,
            20,
            5,
        )
        .unwrap();
    for i in 0..=1 {
        let (conf, tx) = keychain_scan.update.get_tx_in_chain(txids[i]).unwrap();
        assert!(conf.is_confirmed());
        assert_eq!(tx.txid(), txids[i]);
        let full_txout = keychain_scan.update.full_txout(outpoints[i]).unwrap();
        assert!(full_txout.chain_position.is_confirmed());
    }
}

#[tokio::test]
async fn test_async_scanning_stop_gap() {
    let test_framework = TestFramework::init(true, None, None);
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 19);
    let (_, to_script) = revealed_spks.last().unwrap().to_owned();

    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, revealed_spks.into_iter());
    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .await
        .unwrap();
    assert_eq!(
        *keychain_scan
            .last_active_indices
            .get(&Keychain::External)
            .unwrap(),
        19
    );
    let (&conf, chain_tx) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert_eq!(tx, chain_tx.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(103));
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.script_pubkey == to_script)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        });
    assert_eq!(full_txout.unwrap().txout.value, 10000);

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 38);
    let (_, to_script) = revealed_spks.last().unwrap().to_owned();

    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, revealed_spks.into_iter());
    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .await
        .unwrap();
    assert_eq!(
        *keychain_scan
            .last_active_indices
            .get(&Keychain::External)
            .unwrap(),
        38
    );
    let (&conf, chain_tx) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert_eq!(tx, chain_tx.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(104));
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.script_pubkey == to_script)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        });
    assert_eq!(full_txout.unwrap().txout.value, 10000);

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 59);
    let (_, to_script) = revealed_spks.last().unwrap().to_owned();

    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, revealed_spks.into_iter());
    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .await
        .unwrap();
    assert!(keychain_scan
        .last_active_indices
        .get(&Keychain::External)
        .is_none());
    assert!(keychain_scan.update.get_tx_in_chain(tx.txid()).is_none());
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.script_pubkey == to_script)
        .unwrap();
    assert!(keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32
        })
        .is_none());
}

#[tokio::test]
async fn test_async_reorg() -> Result<()> {
    let mut conf = bitcoind::Conf::default();
    conf.p2p = bitcoind::P2P::Yes;
    let test_framework = TestFramework::init(true, Some(conf), None);
    let mut miner_conf = bitcoind::Conf::default();
    miner_conf.p2p = test_framework.bitcoin_daemon.p2p_connect(true).unwrap();
    let bitcoind_exe = env::var("BITCOIND_EXE")
        .ok()
        .or_else(|| bitcoind::downloaded_exe_path().ok())
        .expect(
            "you need to provide an env var BITCOIND_EXE or specify a bitcoind version feature",
        );

    let miner_node = BitcoinD::with_conf(bitcoind_exe, &miner_conf).unwrap();
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let (tx, revealed_spks) =
        send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 0);
    let (_, revealed_spk) = revealed_spks.last().unwrap().to_owned();

    // Get the transaction above confirmed
    test_framework.generate_blocks_and_wait(1);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, [(0, revealed_spk.clone())].into_iter());
    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .await
        .unwrap();
    assert_eq!(
        *keychain_scan
            .last_active_indices
            .get(&Keychain::External)
            .unwrap(),
        0
    );
    let (&conf, chain_tx) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert_eq!(tx, chain_tx.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(103));
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.value == 10000)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        });
    assert_eq!(full_txout.unwrap().txout.value, 10000);

    // Reorg blocks on miner chain
    TestFramework::reorg(3, &miner_node)?;
    // Generate more blocks on the miner node, thereby making it the chain with the most
    // work, so the bitcoin_daemon chain has to catch up on this chain which doesn't
    // have a transaction above.
    let curr_height = miner_node.client.get_block_count().unwrap();
    let address = miner_node
        .client
        .get_new_address(Some("test"), Some(AddressType::Bech32))
        .unwrap();
    let _block_hashes = miner_node
        .client
        .generate_to_address(5u64, &address)
        .unwrap();
    test_framework.wait_for_block(5usize + curr_height as usize);

    let mut spks = BTreeMap::new();
    spks.insert(Keychain::External, [(0, revealed_spk.clone())].into_iter());
    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(&local_chain, spks, [], [], 20, 5)
        .await
        .unwrap();
    let (conf, _tx_chain) = keychain_scan.update.get_tx_in_chain(tx.txid()).unwrap();
    assert!(!conf.is_confirmed());
    let (output_vout, _) = tx
        .output
        .iter()
        .enumerate()
        .find(|(_idx, out)| out.value == 10000)
        .unwrap();
    let full_txout = keychain_scan
        .update
        .full_txout(bdk_chain::bitcoin::OutPoint {
            txid: tx.txid(),
            vout: output_vout as u32,
        })
        .unwrap();
    assert!(!full_txout.chain_position.is_confirmed());

    Ok(())
}

#[tokio::test]
async fn test_async_scan_with_txids() {
    let test_framework = TestFramework::init(true, None, None);
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let (tx_1, _) = send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 0);

    test_framework.generate_blocks_and_wait(1);

    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [tx_1.txid()],
            [],
            20,
            5,
        )
        .await
        .unwrap();

    assert!(keychain_scan
        .last_active_indices
        .get(&Keychain::External)
        .is_none());
    let (conf, tx1_chain) = keychain_scan.update.get_tx_in_chain(tx_1.txid()).unwrap();
    assert_eq!(tx_1, tx1_chain.clone());
    assert!(conf.is_confirmed());
    assert_eq!(conf.height(), TxHeight::Confirmed(103));
    let checkpoint = keychain_scan.update.chain().checkpoint_at(103).unwrap();
    assert_eq!(
        checkpoint.hash,
        test_framework
            .bitcoin_daemon
            .client
            .get_block_hash(103)
            .unwrap()
    );

    let (tx_2, _) = send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, 1);

    test_framework.wait_for_tx_appears_in_esplora(5, &tx_2.txid());
    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [tx_2.txid()],
            [],
            20,
            5,
        )
        .await
        .unwrap();
    let (conf, _tx_chain) = keychain_scan.update.get_tx_in_chain(tx_2.txid()).unwrap();
    assert!(!conf.is_confirmed());
}

#[tokio::test]
async fn test_async_scan_with_outpoints() {
    let test_framework = TestFramework::init(true, None, None);
    test_framework.premine(101);

    let local_chain: BTreeMap<u32, BlockHash> = BTreeMap::new();
    let mut txout_index = init_txout_index();

    let mut outpoints: [OutPoint; 2] = [OutPoint::null(), OutPoint::null()];
    let mut txids: [Txid; 2] = [Txid::from_inner([0x00; 32]), Txid::from_inner([0x00; 32])];
    for i in 0..=1 {
        let (tx, revealed_spks) =
            send_to_revealed_script(&mut txout_index, &test_framework.bitcoin_daemon, i);
        let (_, revealed_spk) = revealed_spks.get(0).unwrap().to_owned();
        let (output_vout, _) = tx
            .output
            .iter()
            .enumerate()
            .find(|(_idx, out)| out.script_pubkey == revealed_spk.clone())
            .unwrap();
        outpoints[i as usize] = OutPoint::new(tx.txid(), output_vout as u32);
        txids[i as usize] = tx.txid();
    }

    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [],
            outpoints,
            20,
            5,
        )
        .await
        .unwrap();
    for i in 0..=1 {
        assert!(keychain_scan.update.get_tx_in_chain(txids[i]).is_none());
        assert!(keychain_scan.update.full_txout(outpoints[i]).is_none());
    }

    test_framework.generate_blocks_and_wait(1);

    let keychain_scan = test_framework
        .async_client
        .as_ref()
        .unwrap()
        .scan(
            &local_chain,
            BTreeMap::<Keychain, Vec<(u32, Script)>>::new(),
            [],
            outpoints,
            20,
            5,
        )
        .await
        .unwrap();
    for i in 0..=1 {
        let (conf, tx) = keychain_scan.update.get_tx_in_chain(txids[i]).unwrap();
        assert!(conf.is_confirmed());
        assert_eq!(tx.txid(), txids[i]);
        let full_txout = keychain_scan.update.full_txout(outpoints[i]).unwrap();
        assert!(full_txout.chain_position.is_confirmed());
    }
}

fn init_txout_index() -> KeychainTxOutIndex<Keychain> {
    let mut txout_index = KeychainTxOutIndex::<Keychain>::default();
    let secp = bdk_chain::bitcoin::secp256k1::Secp256k1::default();
    let (external_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/0/*)").unwrap();
    let (internal_descriptor,_) = Descriptor::<DescriptorPublicKey>::parse_descriptor(&secp, "tr([73c5da0a/86'/0'/0']xprv9xgqHN7yz9MwCkxsBPN5qetuNdQSUttZNKw1dcYTV4mkaAFiBVGQziHs3NRSWMkCzvgjEe3n9xV8oYywvM8at9yRqyaZVz6TYYhX98VjsUk/1/*)").unwrap();

    txout_index.add_keychain(Keychain::External, external_descriptor);
    txout_index.add_keychain(Keychain::Internal, internal_descriptor);
    txout_index
}

fn send_to_revealed_script(
    txout_index: &mut KeychainTxOutIndex<Keychain>,
    bitcoin_daemon: &BitcoinD,
    reveal_target: u32,
) -> (Transaction, Vec<(u32, Script)>) {
    let revealed_spks = txout_index
        .reveal_to_target(&Keychain::External, reveal_target)
        .0
        .collect::<Vec<(u32, Script)>>();

    let (_idx, script) = revealed_spks.last().unwrap().to_owned();
    let address = Address::from_script(&script, Regtest).unwrap();
    let txid = bitcoin_daemon
        .client
        .send_to_address(
            &address,
            Amount::from_sat(10000),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

    (
        bitcoin_daemon
            .client
            .get_transaction(&txid, Some(false))
            .unwrap()
            .transaction()
            .unwrap(),
        revealed_spks,
    )
}
