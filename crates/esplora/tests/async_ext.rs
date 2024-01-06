use bdk_esplora::EsploraAsyncExt;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use electrsd::bitcoind::{self, anyhow, BitcoinD};
use electrsd::electrum_client::ElectrumApi;
use electrsd::{Conf, ElectrsD};
use esplora_client::{self, AsyncClient, Builder};
use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use bdk_chain::bitcoin::{Address, Amount, BlockHash, Txid};

struct TestEnv {
    bitcoind: BitcoinD,
    #[allow(dead_code)]
    electrsd: ElectrsD,
    client: AsyncClient,
}

impl TestEnv {
    fn new() -> Result<Self, anyhow::Error> {
        let bitcoind_exe =
            bitcoind::exe_path().expect("Cannot find bitcoind daemon, set BITCOIND_EXEC environment variable with the path to bitcoind");
        let mut bitcoind_conf = bitcoind::Conf::default();
        bitcoind_conf.p2p = bitcoind::P2P::Yes;
        let bitcoind = BitcoinD::with_conf(bitcoind_exe, &bitcoind_conf)?;

        let mut electrs_conf = Conf::default();
        electrs_conf.http_enabled = true;
        let electrs_exe = electrsd::exe_path().expect("Cannot find electrs daemon, set ELECTRS_EXEC environment variable with the path to electrs");
        let electrsd = ElectrsD::with_conf(electrs_exe, &bitcoind, &electrs_conf)?;

        // Alive checks
        bitcoind.client.ping().unwrap(); // without using bitcoind, it is dropped and all the rest fails.
        electrsd.client.ping().unwrap();
        assert!(bitcoind.client.ping().is_ok());
        assert!(electrsd.client.ping().is_ok());

        let base_url = format!("http://{}", &electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_async()?;

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

#[tokio::test]
pub async fn test_update_tx_graph_without_keychain() -> anyhow::Result<()> {
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
    while env.client.get_height().await.unwrap() < 102 {
        sleep(Duration::from_millis(10))
    }

    let graph_update = env
        .client
        .sync(
            misc_spks.into_iter(),
            vec![].into_iter(),
            vec![].into_iter(),
            1,
        )
        .await?;

    // Check to see if we have the floating txouts available from our two created transactions'
    // previous outputs in order to calculate transaction fees.
    for tx in graph_update.full_txs() {
        // Retrieve the calculated fee from `TxGraph`, which will panic if we do not have the
        // floating txouts available from the transactions' previous outputs.
        let fee = graph_update.calculate_fee(tx.tx).expect("Fee must exist");

        // Retrieve the fee in the transaction data from `bitcoind`.
        let tx_fee = env
            .bitcoind
            .client
            .get_transaction(&tx.txid, None)
            .expect("Tx must exist")
            .fee
            .expect("Fee must exist")
            .abs()
            .to_sat() as u64;

        // Check that the calculated fee matches the fee from the transaction data.
        assert_eq!(fee, tx_fee);
    }

    let mut graph_update_txids: Vec<Txid> = graph_update.full_txs().map(|tx| tx.txid).collect();
    graph_update_txids.sort();
    let mut expected_txids = vec![txid1, txid2];
    expected_txids.sort();
    assert_eq!(graph_update_txids, expected_txids);
    Ok(())
}

/// Test the bounds of the address scan depending on the gap limit.
#[tokio::test]
pub async fn test_async_update_tx_graph_gap_limit() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let _block_hashes = env.mine_blocks(101, None)?;

    // Now let's test the gap limit. First of all get a chain of 10 addresses.
    let addresses = [
        "bcrt1qj9f7r8r3p2y0sqf4r3r62qysmkuh0fzep473d2ar7rcz64wqvhssjgf0z4",
        "bcrt1qmm5t0ch7vh2hryx9ctq3mswexcugqe4atkpkl2tetm8merqkthas3w7q30",
        "bcrt1qut9p7ej7l7lhyvekj28xknn8gnugtym4d5qvnp5shrsr4nksmfqsmyn87g",
        "bcrt1qqz0xtn3m235p2k96f5wa2dqukg6shxn9n3txe8arlrhjh5p744hsd957ww",
        "bcrt1q9c0t62a8l6wfytmf2t9lfj35avadk3mm8g4p3l84tp6rl66m48sqrme7wu",
        "bcrt1qkmh8yrk2v47cklt8dytk8f3ammcwa4q7dzattedzfhqzvfwwgyzsg59zrh",
        "bcrt1qvgrsrzy07gjkkfr5luplt0azxtfwmwq5t62gum5jr7zwcvep2acs8hhnp2",
        "bcrt1qw57edarcg50ansq8mk3guyrk78rk0fwvrds5xvqeupteu848zayq549av8",
        "bcrt1qvtve5ekf6e5kzs68knvnt2phfw6a0yjqrlgat392m6zt9jsvyxhqfx67ef",
        "bcrt1qw03ddumfs9z0kcu76ln7jrjfdwam20qtffmkcral3qtza90sp9kqm787uk",
    ];
    let addresses: Vec<_> = addresses
        .into_iter()
        .map(|s| Address::from_str(s).unwrap().assume_checked())
        .collect();
    let spks: Vec<_> = addresses
        .iter()
        .enumerate()
        .map(|(i, addr)| (i as u32, addr.script_pubkey()))
        .collect();
    let mut keychains = BTreeMap::new();
    keychains.insert(0, spks);

    // Then receive coins on the 4th address.
    let txid_4th_addr = env.bitcoind.client.send_to_address(
        &addresses[3],
        Amount::from_sat(10000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while env.client.get_height().await.unwrap() < 103 {
        sleep(Duration::from_millis(10))
    }

    // A scan with a gap limit of 2 won't find the transaction, but a scan with a gap limit of 3
    // will.
    let (graph_update, active_indices) = env.client.full_scan(keychains.clone(), 2, 1).await?;
    assert!(graph_update.full_txs().next().is_none());
    assert!(active_indices.is_empty());
    let (graph_update, active_indices) = env.client.full_scan(keychains.clone(), 3, 1).await?;
    assert_eq!(graph_update.full_txs().next().unwrap().txid, txid_4th_addr);
    assert_eq!(active_indices[&0], 3);

    // Now receive a coin on the last address.
    let txid_last_addr = env.bitcoind.client.send_to_address(
        &addresses[addresses.len() - 1],
        Amount::from_sat(10000),
        None,
        None,
        None,
        None,
        Some(1),
        None,
    )?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while env.client.get_height().await.unwrap() < 104 {
        sleep(Duration::from_millis(10))
    }

    // A scan with gap limit 4 won't find the second transaction, but a scan with gap limit 5 will.
    // The last active indice won't be updated in the first case but will in the second one.
    let (graph_update, active_indices) = env.client.full_scan(keychains.clone(), 4, 1).await?;
    let txs: HashSet<_> = graph_update.full_txs().map(|tx| tx.txid).collect();
    assert_eq!(txs.len(), 1);
    assert!(txs.contains(&txid_4th_addr));
    assert_eq!(active_indices[&0], 3);
    let (graph_update, active_indices) = env.client.full_scan(keychains, 5, 1).await?;
    let txs: HashSet<_> = graph_update.full_txs().map(|tx| tx.txid).collect();
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&txid_4th_addr) && txs.contains(&txid_last_addr));
    assert_eq!(active_indices[&0], 9);

    Ok(())
}
