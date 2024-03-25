use bdk_esplora::EsploraAsyncExt;
use electrsd::bitcoind::anyhow;
use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use esplora_client::{self, Builder};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

use bdk_chain::bitcoin::{Address, Amount, Txid};
use bdk_testenv::TestEnv;

#[tokio::test]
pub async fn test_update_tx_graph_without_keychain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_async()?;

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
    while client.get_height().await.unwrap() < 102 {
        sleep(Duration::from_millis(10))
    }

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    let sync_update = client
        .sync(
            cp_tip.clone(),
            misc_spks.into_iter(),
            vec![].into_iter(),
            vec![].into_iter(),
            1,
        )
        .await?;

    assert!(
        {
            let update_cps = sync_update
                .local_chain
                .tip
                .iter()
                .map(|cp| cp.block_id())
                .collect::<BTreeSet<_>>();
            let superset_cps = cp_tip
                .iter()
                .map(|cp| cp.block_id())
                .collect::<BTreeSet<_>>();
            superset_cps.is_superset(&update_cps)
        },
        "update should not alter original checkpoint tip since we already started with all checkpoints",
    );

    let graph_update = sync_update.tx_graph;
    // Check to see if we have the floating txouts available from our two created transactions'
    // previous outputs in order to calculate transaction fees.
    for tx in graph_update.full_txs() {
        // Retrieve the calculated fee from `TxGraph`, which will panic if we do not have the
        // floating txouts available from the transactions' previous outputs.
        let fee = graph_update.calculate_fee(&tx.tx).expect("Fee must exist");

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

/// Test the bounds of the address scan depending on the `stop_gap`.
#[tokio::test]
pub async fn test_async_update_tx_graph_stop_gap() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_async()?;
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
    while client.get_height().await.unwrap() < 103 {
        sleep(Duration::from_millis(10))
    }

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    // A scan with a gap limit of 3 won't find the transaction, but a scan with a gap limit of 4
    // will.
    let full_scan_update = client
        .full_scan(cp_tip.clone(), keychains.clone(), 3, 1)
        .await?;
    assert!(full_scan_update.tx_graph.full_txs().next().is_none());
    assert!(full_scan_update.last_active_indices.is_empty());
    let full_scan_update = client
        .full_scan(cp_tip.clone(), keychains.clone(), 4, 1)
        .await?;
    assert_eq!(
        full_scan_update.tx_graph.full_txs().next().unwrap().txid,
        txid_4th_addr
    );
    assert_eq!(full_scan_update.last_active_indices[&0], 3);

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
    while client.get_height().await.unwrap() < 104 {
        sleep(Duration::from_millis(10))
    }

    // A scan with gap limit 5 won't find the second transaction, but a scan with gap limit 6 will.
    // The last active indice won't be updated in the first case but will in the second one.
    let full_scan_update = client
        .full_scan(cp_tip.clone(), keychains.clone(), 5, 1)
        .await?;
    let txs: HashSet<_> = full_scan_update
        .tx_graph
        .full_txs()
        .map(|tx| tx.txid)
        .collect();
    assert_eq!(txs.len(), 1);
    assert!(txs.contains(&txid_4th_addr));
    assert_eq!(full_scan_update.last_active_indices[&0], 3);
    let full_scan_update = client.full_scan(cp_tip, keychains, 6, 1).await?;
    let txs: HashSet<_> = full_scan_update
        .tx_graph
        .full_txs()
        .map(|tx| tx.txid)
        .collect();
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&txid_4th_addr) && txs.contains(&txid_last_addr));
    assert_eq!(full_scan_update.last_active_indices[&0], 9);

    Ok(())
}
