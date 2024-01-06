use bdk_chain::local_chain::LocalChain;
use bdk_chain::BlockId;
use bdk_esplora::EsploraExt;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use electrsd::bitcoind::{self, anyhow, BitcoinD};
use electrsd::electrum_client::ElectrumApi;
use electrsd::{Conf, ElectrsD};
use esplora_client::{self, BlockingClient, Builder};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use bdk_chain::bitcoin::{Address, Amount, BlockHash, Txid};

macro_rules! h {
    ($index:literal) => {{
        bdk_chain::bitcoin::hashes::Hash::hash($index.as_bytes())
    }};
}

macro_rules! local_chain {
    [ $(($height:expr, $block_hash:expr)), * ] => {{
        #[allow(unused_mut)]
        bdk_chain::local_chain::LocalChain::from_blocks([$(($height, $block_hash).into()),*].into_iter().collect())
            .expect("chain must have genesis block")
    }};
}

struct TestEnv {
    bitcoind: BitcoinD,
    #[allow(dead_code)]
    electrsd: ElectrsD,
    client: BlockingClient,
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
        let client = Builder::new(base_url.as_str()).build_blocking()?;

        Ok(Self {
            bitcoind,
            electrsd,
            client,
        })
    }

    fn reset_electrsd(mut self) -> anyhow::Result<Self> {
        let mut electrs_conf = Conf::default();
        electrs_conf.http_enabled = true;
        let electrs_exe = electrsd::exe_path().expect("Cannot find electrs daemon, set ELECTRS_EXEC environment variable with the path to electrs");
        let electrsd = ElectrsD::with_conf(electrs_exe, &self.bitcoind, &electrs_conf)?;

        let base_url = format!("http://{}", &electrsd.esplora_url.clone().unwrap());
        let client = Builder::new(base_url.as_str()).build_blocking()?;
        self.electrsd = electrsd;
        self.client = client;
        Ok(self)
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

    let graph_update = env.client.sync(
        misc_spks.into_iter(),
        vec![].into_iter(),
        vec![].into_iter(),
        1,
    )?;

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
#[test]
pub fn test_update_tx_graph_gap_limit() -> anyhow::Result<()> {
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
    while env.client.get_height().unwrap() < 103 {
        sleep(Duration::from_millis(10))
    }

    // A scan with a gap limit of 2 won't find the transaction, but a scan with a gap limit of 3
    // will.
    let (graph_update, active_indices) = env.client.full_scan(keychains.clone(), 2, 1)?;
    assert!(graph_update.full_txs().next().is_none());
    assert!(active_indices.is_empty());
    let (graph_update, active_indices) = env.client.full_scan(keychains.clone(), 3, 1)?;
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
    while env.client.get_height().unwrap() < 104 {
        sleep(Duration::from_millis(10))
    }

    // A scan with gap limit 4 won't find the second transaction, but a scan with gap limit 5 will.
    // The last active indice won't be updated in the first case but will in the second one.
    let (graph_update, active_indices) = env.client.full_scan(keychains.clone(), 4, 1)?;
    let txs: HashSet<_> = graph_update.full_txs().map(|tx| tx.txid).collect();
    assert_eq!(txs.len(), 1);
    assert!(txs.contains(&txid_4th_addr));
    assert_eq!(active_indices[&0], 3);
    let (graph_update, active_indices) = env.client.full_scan(keychains, 5, 1)?;
    let txs: HashSet<_> = graph_update.full_txs().map(|tx| tx.txid).collect();
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&txid_4th_addr) && txs.contains(&txid_last_addr));
    assert_eq!(active_indices[&0], 9);

    Ok(())
}

#[test]
fn update_local_chain() -> anyhow::Result<()> {
    const TIP_HEIGHT: u32 = 50;

    let env = TestEnv::new()?;
    let blocks = {
        let bitcoind_client = &env.bitcoind.client;
        assert_eq!(bitcoind_client.get_block_count()?, 1);
        [
            (0, bitcoind_client.get_block_hash(0)?),
            (1, bitcoind_client.get_block_hash(1)?),
        ]
        .into_iter()
        .chain((2..).zip(env.mine_blocks((TIP_HEIGHT - 1) as usize, None)?))
        .collect::<BTreeMap<_, _>>()
    };
    // so new blocks can be seen by Electrs
    let env = env.reset_electrsd()?;

    struct TestCase {
        name: &'static str,
        chain: LocalChain,
        request_heights: &'static [u32],
        exp_update_heights: &'static [u32],
    }

    let test_cases = [
        TestCase {
            name: "request_later_blocks",
            chain: local_chain![(0, blocks[&0]), (21, blocks[&21])],
            request_heights: &[22, 25, 28],
            exp_update_heights: &[21, 22, 25, 28],
        },
        TestCase {
            name: "request_prev_blocks",
            chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (5, blocks[&5])],
            request_heights: &[4],
            exp_update_heights: &[4, 5],
        },
        TestCase {
            name: "request_prev_blocks_2",
            chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (10, blocks[&10])],
            request_heights: &[4, 6],
            exp_update_heights: &[4, 6, 10],
        },
        TestCase {
            name: "request_later_and_prev_blocks",
            chain: local_chain![(0, blocks[&0]), (7, blocks[&7]), (11, blocks[&11])],
            request_heights: &[8, 9, 15],
            exp_update_heights: &[8, 9, 11, 15],
        },
        TestCase {
            name: "request_tip_only",
            chain: local_chain![(0, blocks[&0]), (5, blocks[&5]), (49, blocks[&49])],
            request_heights: &[TIP_HEIGHT],
            exp_update_heights: &[49],
        },
        TestCase {
            name: "request_nothing",
            chain: local_chain![(0, blocks[&0]), (13, blocks[&13]), (23, blocks[&23])],
            request_heights: &[],
            exp_update_heights: &[23],
        },
        TestCase {
            name: "request_nothing_during_reorg",
            chain: local_chain![(0, blocks[&0]), (13, blocks[&13]), (23, h!("23"))],
            request_heights: &[],
            exp_update_heights: &[13, 23],
        },
        TestCase {
            name: "request_nothing_during_reorg_2",
            chain: local_chain![
                (0, blocks[&0]),
                (21, blocks[&21]),
                (22, h!("22")),
                (23, h!("23"))
            ],
            request_heights: &[],
            exp_update_heights: &[21, 22, 23],
        },
        TestCase {
            name: "request_prev_blocks_during_reorg",
            chain: local_chain![
                (0, blocks[&0]),
                (21, blocks[&21]),
                (22, h!("22")),
                (23, h!("23"))
            ],
            request_heights: &[17, 20],
            exp_update_heights: &[17, 20, 21, 22, 23],
        },
        TestCase {
            name: "request_later_blocks_during_reorg",
            chain: local_chain![
                (0, blocks[&0]),
                (9, blocks[&9]),
                (22, h!("22")),
                (23, h!("23"))
            ],
            request_heights: &[25, 27],
            exp_update_heights: &[9, 22, 23, 25, 27],
        },
        TestCase {
            name: "request_later_blocks_during_reorg_2",
            chain: local_chain![(0, blocks[&0]), (9, h!("9"))],
            request_heights: &[10],
            exp_update_heights: &[0, 9, 10],
        },
        TestCase {
            name: "request_later_and_prev_blocks_during_reorg",
            chain: local_chain![(0, blocks[&0]), (1, blocks[&1]), (9, h!("9"))],
            request_heights: &[8, 11],
            exp_update_heights: &[1, 8, 9, 11],
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        println!("Case {}: {}", i, t.name);
        let mut chain = t.chain;

        let update = env
            .client
            .update_local_chain(chain.tip(), t.request_heights.iter().copied())
            .map_err(|err| {
                anyhow::format_err!("[{}:{}] `update_local_chain` failed: {}", i, t.name, err)
            })?;

        let update_blocks = update
            .tip
            .iter()
            .map(|cp| cp.block_id())
            .collect::<BTreeSet<_>>();

        let exp_update_blocks = t
            .exp_update_heights
            .iter()
            .map(|&height| {
                let hash = blocks[&height];
                BlockId { height, hash }
            })
            .chain(
                // Electrs Esplora `get_block` call fetches 10 blocks which is included in the
                // update
                blocks
                    .range(TIP_HEIGHT - 9..)
                    .map(|(&height, &hash)| BlockId { height, hash }),
            )
            .collect::<BTreeSet<_>>();

        assert_eq!(
            update_blocks, exp_update_blocks,
            "[{}:{}] unexpected update",
            i, t.name
        );

        let _ = chain
            .apply_update(update)
            .unwrap_or_else(|err| panic!("[{}:{}] update failed to apply: {}", i, t.name, err));

        // all requested heights must exist in the final chain
        for height in t.request_heights {
            let exp_blockhash = blocks.get(height).expect("block must exist in bitcoind");
            assert_eq!(
                chain.blocks().get(height),
                Some(exp_blockhash),
                "[{}:{}] block {}:{} must exist in final chain",
                i,
                t.name,
                height,
                exp_blockhash
            );
        }
    }

    Ok(())
}
