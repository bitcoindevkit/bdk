use bdk_esplora::EsploraExt;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use electrsd::bitcoind::{self, anyhow, BitcoinD};
use electrsd::{Conf, ElectrsD};
use esplora_client::{self, BlockingClient, Builder};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use bdk_chain::bitcoin::{Address, Amount, BlockHash, Txid};

struct TestEnv {
    bitcoind: BitcoinD,
    #[allow(dead_code)]
    electrsd: ElectrsD,
    client: BlockingClient,
}

impl TestEnv {
    fn new() -> Result<Self, anyhow::Error> {
        let bitcoind_exe =
            bitcoind::downloaded_exe_path().expect("bitcoind version feature must be enabled");
        let bitcoind = BitcoinD::new(bitcoind_exe).unwrap();

        let mut electrs_conf = Conf::default();
        electrs_conf.http_enabled = true;
        let electrs_exe =
            electrsd::downloaded_exe_path().expect("electrs version feature must be enabled");
        let electrsd = ElectrsD::with_conf(electrs_exe, &bitcoind, &electrs_conf)?;

        let base_url = format!("http://{}", &electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_blocking()?;

        Ok(Self {
            bitcoind,
            electrsd,
            client,
        })
    }

    fn mine_blocks(
        &self,
        count: usize,
        address: Option<Address>,
    ) -> anyhow::Result<Vec<BlockHash>> {
        let coinbase_address = match address {
            Some(address) => address,
            None => self
                .bitcoind
                .client
                .get_new_address(None, None)?
                .assume_checked(),
        };
        let block_hashes = self
            .bitcoind
            .client
            .generate_to_address(count as _, &coinbase_address)?;
        Ok(block_hashes)
    }
}

#[test]
pub fn test_update_tx_graph_without_keychain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let receive_address0 =
        Address::from_str("bcrt1qc6fweuf4xjvz4x3gx3t9e0fh4hvqyu2qw4wvxm")?.assume_checked();
    let receive_address1 =
        Address::from_str("bcrt1qfjg5lv3dvc9az8patec8fjddrs4aqtauadnagr")?.assume_checked();

    let misc_spks = [
        receive_address0.script_pubkey(),
        receive_address1.script_pubkey(),
    ];

    let _block_hashes = env.mine_blocks(101, None)?;
    let txid1 = env.bitcoind.client.send_to_address(
        &receive_address1,
        Amount::from_sat(10000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let txid2 = env.bitcoind.client.send_to_address(
        &receive_address0,
        Amount::from_sat(20000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while env.client.get_height().unwrap() < 102 {
        sleep(Duration::from_millis(10))
    }

    let graph_update = env.client.scan_txs(
        misc_spks.into_iter(),
        vec![].into_iter(),
        vec![].into_iter(),
        1,
    )?;

    let mut graph_update_txids: Vec<Txid> = graph_update.full_txs().map(|tx| tx.txid).collect();
    graph_update_txids.sort();
    let mut expected_txids = vec![txid1, txid2];
    expected_txids.sort();
    assert_eq!(graph_update_txids, expected_txids);
    Ok(())
}
