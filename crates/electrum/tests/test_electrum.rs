use bdk_chain::{
    bitcoin::{hashes::Hash, Address, Amount, ScriptBuf, WScriptHash},
    local_chain::LocalChain,
    spk_client::{FullScanRequest, SyncRequest, SyncResponse},
    spk_txout::SpkTxOutIndex,
    Balance, CanonicalizationMods, ConfirmationBlockTime, IndexedTxGraph, Indexer, Merge, TxGraph,
};
use bdk_electrum::BdkElectrumClient;
use bdk_testenv::{anyhow, bitcoincore_rpc::RpcApi, TestEnv};
use core::time::Duration;
use std::collections::{BTreeSet, HashSet};
use std::str::FromStr;

// Batch size for `sync_with_electrum`.
const BATCH_SIZE: usize = 5;

fn get_balance(
    recv_chain: &LocalChain,
    recv_graph: &IndexedTxGraph<ConfirmationBlockTime, SpkTxOutIndex<()>>,
) -> anyhow::Result<Balance> {
    let chain_tip = recv_chain.tip().block_id();
    let outpoints = recv_graph.index.outpoints().clone();
    let balance = recv_graph.graph().balance(
        recv_chain,
        chain_tip,
        CanonicalizationMods::NONE,
        outpoints,
        |_, _| true,
    );
    Ok(balance)
}

fn sync_with_electrum<I, Spks>(
    client: &BdkElectrumClient<electrum_client::Client>,
    spks: Spks,
    chain: &mut LocalChain,
    graph: &mut IndexedTxGraph<ConfirmationBlockTime, I>,
) -> anyhow::Result<SyncResponse>
where
    I: Indexer,
    I::ChangeSet: Default + Merge,
    Spks: IntoIterator<Item = ScriptBuf>,
    Spks::IntoIter: ExactSizeIterator + Send + 'static,
{
    let update = client.sync(
        SyncRequest::builder().chain_tip(chain.tip()).spks(spks),
        BATCH_SIZE,
        true,
    )?;

    if let Some(chain_update) = update.chain_update.clone() {
        let _ = chain
            .apply_update(chain_update)
            .map_err(|err| anyhow::anyhow!("LocalChain update error: {:?}", err))?;
    }
    let _ = graph.apply_update(update.tx_update.clone());

    Ok(update)
}

#[test]
pub fn test_update_tx_graph_without_keychain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str())?;
    let client = BdkElectrumClient::new(electrum_client);

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
    env.mine_blocks(1, None)?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    let sync_update = {
        let request = SyncRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks(misc_spks);
        client.sync(request, 1, true)?
    };

    assert!(
        {
            let update_cps = sync_update
                .chain_update
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

    let tx_update = sync_update.tx_update;
    let updated_graph = {
        let mut graph = TxGraph::<ConfirmationBlockTime>::default();
        let _ = graph.apply_update(tx_update.clone());
        graph
    };
    // Check to see if we have the floating txouts available from our two created transactions'
    // previous outputs in order to calculate transaction fees.
    for tx in &tx_update.txs {
        // Retrieve the calculated fee from `TxGraph`, which will panic if we do not have the
        // floating txouts available from the transactions' previous outputs.
        let fee = updated_graph.calculate_fee(tx).expect("Fee must exist");

        // Retrieve the fee in the transaction data from `bitcoind`.
        let tx_fee = env
            .bitcoind
            .client
            .get_transaction(&tx.compute_txid(), None)
            .expect("Tx must exist")
            .fee
            .expect("Fee must exist")
            .abs()
            .to_unsigned()
            .expect("valid `Amount`");

        // Check that the calculated fee matches the fee from the transaction data.
        assert_eq!(fee, tx_fee);
    }

    assert_eq!(
        tx_update
            .txs
            .iter()
            .map(|tx| tx.compute_txid())
            .collect::<BTreeSet<_>>(),
        [txid1, txid2].into(),
        "update must include all expected transactions",
    );
    Ok(())
}

/// Test the bounds of the address scan depending on the `stop_gap`.
#[test]
pub fn test_update_tx_graph_stop_gap() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str())?;
    let client = BdkElectrumClient::new(electrum_client);
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
    env.mine_blocks(1, None)?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    // A scan with a stop_gap of 3 won't find the transaction, but a scan with a gap limit of 4
    // will.
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 3, 1, false)?
    };
    assert!(full_scan_update.tx_update.txs.is_empty());
    assert!(full_scan_update.last_active_indices.is_empty());
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 4, 1, false)?
    };
    assert_eq!(
        full_scan_update
            .tx_update
            .txs
            .first()
            .unwrap()
            .compute_txid(),
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
    env.mine_blocks(1, None)?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    // A scan with gap limit 5 won't find the second transaction, but a scan with gap limit 6 will.
    // The last active indice won't be updated in the first case but will in the second one.
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 5, 1, false)?
    };
    let txs: HashSet<_> = full_scan_update
        .tx_update
        .txs
        .iter()
        .map(|tx| tx.compute_txid())
        .collect();
    assert_eq!(txs.len(), 1);
    assert!(txs.contains(&txid_4th_addr));
    assert_eq!(full_scan_update.last_active_indices[&0], 3);
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 6, 1, false)?
    };
    let txs: HashSet<_> = full_scan_update
        .tx_update
        .txs
        .iter()
        .map(|tx| tx.compute_txid())
        .collect();
    assert_eq!(txs.len(), 2);
    assert!(txs.contains(&txid_4th_addr) && txs.contains(&txid_last_addr));
    assert_eq!(full_scan_update.last_active_indices[&0], 9);

    Ok(())
}

/// Ensure that [`BdkElectrumClient::sync`] can confirm previously unconfirmed transactions in both
/// reorg and no-reorg situations. After the transaction is confirmed after reorg, check if floating
/// txouts for previous outputs were inserted for transaction fee calculation.
#[test]
fn test_sync() -> anyhow::Result<()> {
    const SEND_AMOUNT: Amount = Amount::from_sat(10_000);

    let env = TestEnv::new()?;
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str())?;
    let client = BdkElectrumClient::new(electrum_client);

    // Setup addresses.
    let addr_to_mine = env
        .bitcoind
        .client
        .get_new_address(None, None)?
        .assume_checked();
    let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, bdk_chain::bitcoin::Network::Regtest)?;

    // Setup receiver.
    let (mut recv_chain, _) = LocalChain::from_genesis_hash(env.bitcoind.client.get_block_hash(0)?);
    let mut recv_graph = IndexedTxGraph::<ConfirmationBlockTime, _>::new({
        let mut recv_index = SpkTxOutIndex::default();
        recv_index.insert_spk((), spk_to_track.clone());
        recv_index
    });

    // Mine some blocks.
    env.mine_blocks(101, Some(addr_to_mine))?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    // Broadcast transaction to mempool.
    let txid = env.send(&addr_to_track, SEND_AMOUNT)?;
    env.wait_until_electrum_sees_txid(txid, Duration::from_secs(6))?;

    let _ = sync_with_electrum(
        &client,
        [spk_to_track.clone()],
        &mut recv_chain,
        &mut recv_graph,
    )?;

    // Check for unconfirmed balance when transaction exists only in mempool.
    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            trusted_pending: SEND_AMOUNT,
            ..Balance::default()
        },
        "balance must be correct",
    );

    // Mine block to confirm transaction.
    env.mine_blocks(1, None)?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    let _ = sync_with_electrum(
        &client,
        [spk_to_track.clone()],
        &mut recv_chain,
        &mut recv_graph,
    )?;

    // Check if balance is correct when transaction is confirmed.
    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            confirmed: SEND_AMOUNT,
            ..Balance::default()
        },
        "balance must be correct",
    );

    // Perform reorg on block with confirmed transaction.
    env.reorg_empty_blocks(1)?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    let _ = sync_with_electrum(
        &client,
        [spk_to_track.clone()],
        &mut recv_chain,
        &mut recv_graph,
    )?;

    // Check if balance is correct when transaction returns to mempool.
    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            trusted_pending: SEND_AMOUNT,
            ..Balance::default()
        },
    );

    // Mine block to confirm transaction again.
    env.mine_blocks(1, None)?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    let _ = sync_with_electrum(&client, [spk_to_track], &mut recv_chain, &mut recv_graph)?;

    // Check if balance is correct once transaction is confirmed again.
    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            confirmed: SEND_AMOUNT,
            ..Balance::default()
        },
        "balance must be correct",
    );

    // Check to see if we have the floating txouts available from our transactions' previous outputs
    // in order to calculate transaction fees.
    for tx in recv_graph.graph().full_txs() {
        // Retrieve the calculated fee from `TxGraph`, which will panic if we do not have the
        // floating txouts available from the transaction's previous outputs.
        let fee = recv_graph
            .graph()
            .calculate_fee(&tx.tx)
            .expect("fee must exist");

        // Retrieve the fee in the transaction data from `bitcoind`.
        let tx_fee = env
            .bitcoind
            .client
            .get_transaction(&tx.txid, None)
            .expect("Tx must exist")
            .fee
            .expect("Fee must exist")
            .abs()
            .to_unsigned()
            .expect("valid `Amount`");

        // Check that the calculated fee matches the fee from the transaction data.
        assert_eq!(fee, tx_fee);
    }

    Ok(())
}

/// Ensure that confirmed txs that are reorged become unconfirmed.
///
/// 1. Mine 101 blocks.
/// 2. Mine 8 blocks with a confirmed tx in each.
/// 3. Perform 8 separate reorgs on each block with a confirmed tx.
/// 4. Check [`Balance`] after each reorg to ensure unconfirmed amount is correct.
#[test]
fn tx_can_become_unconfirmed_after_reorg() -> anyhow::Result<()> {
    const REORG_COUNT: usize = 8;
    const SEND_AMOUNT: Amount = Amount::from_sat(10_000);

    let env = TestEnv::new()?;
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str())?;
    let client = BdkElectrumClient::new(electrum_client);

    // Setup addresses.
    let addr_to_mine = env
        .bitcoind
        .client
        .get_new_address(None, None)?
        .assume_checked();
    let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, bdk_chain::bitcoin::Network::Regtest)?;

    // Setup receiver.
    let (mut recv_chain, _) = LocalChain::from_genesis_hash(env.bitcoind.client.get_block_hash(0)?);
    let mut recv_graph = IndexedTxGraph::<ConfirmationBlockTime, _>::new({
        let mut recv_index = SpkTxOutIndex::default();
        recv_index.insert_spk((), spk_to_track.clone());
        recv_index
    });

    // Mine some blocks.
    env.mine_blocks(101, Some(addr_to_mine))?;

    // Create transactions that are tracked by our receiver.
    let mut txids = vec![];
    let mut hashes = vec![];
    for _ in 0..REORG_COUNT {
        txids.push(env.send(&addr_to_track, SEND_AMOUNT)?);
        hashes.extend(env.mine_blocks(1, None)?);
    }

    // Sync up to tip.
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;
    let update = sync_with_electrum(
        &client,
        [spk_to_track.clone()],
        &mut recv_chain,
        &mut recv_graph,
    )?;

    // Retain a snapshot of all anchors before reorg process.
    let initial_anchors = update.tx_update.anchors.clone();
    assert_eq!(initial_anchors.len(), REORG_COUNT);
    for i in 0..REORG_COUNT {
        let (anchor, txid) = initial_anchors.iter().nth(i).unwrap();
        assert_eq!(anchor.block_id.hash, hashes[i]);
        assert_eq!(*txid, txids[i]);
    }

    // Check if initial balance is correct.
    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            confirmed: SEND_AMOUNT * REORG_COUNT as u64,
            ..Balance::default()
        },
        "initial balance must be correct",
    );

    // Perform reorgs with different depths.
    for depth in 1..=REORG_COUNT {
        env.reorg_empty_blocks(depth)?;

        env.wait_until_electrum_sees_block(Duration::from_secs(6))?;
        let update = sync_with_electrum(
            &client,
            [spk_to_track.clone()],
            &mut recv_chain,
            &mut recv_graph,
        )?;

        // Check that no new anchors are added during current reorg.
        assert!(initial_anchors.is_superset(&update.tx_update.anchors));

        assert_eq!(
            get_balance(&recv_chain, &recv_graph)?,
            Balance {
                trusted_pending: SEND_AMOUNT * depth as u64,
                confirmed: SEND_AMOUNT * (REORG_COUNT - depth) as u64,
                ..Balance::default()
            },
            "reorg_count: {}",
            depth,
        );
    }

    Ok(())
}

#[test]
fn test_sync_with_coinbase() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str())?;
    let client = BdkElectrumClient::new(electrum_client);

    // Setup address.
    let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, bdk_chain::bitcoin::Network::Regtest)?;

    // Setup receiver.
    let (mut recv_chain, _) = LocalChain::from_genesis_hash(env.bitcoind.client.get_block_hash(0)?);
    let mut recv_graph = IndexedTxGraph::<ConfirmationBlockTime, _>::new({
        let mut recv_index = SpkTxOutIndex::default();
        recv_index.insert_spk((), spk_to_track.clone());
        recv_index
    });

    // Mine some blocks.
    env.mine_blocks(101, Some(addr_to_track))?;
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;

    // Check to see if electrum syncs properly.
    assert!(sync_with_electrum(
        &client,
        [spk_to_track.clone()],
        &mut recv_chain,
        &mut recv_graph,
    )
    .is_ok());

    Ok(())
}

#[test]
fn test_check_fee_calculation() -> anyhow::Result<()> {
    const SEND_AMOUNT: Amount = Amount::from_sat(10_000);
    const FEE_AMOUNT: Amount = Amount::from_sat(1650);
    let env = TestEnv::new()?;
    let electrum_client = electrum_client::Client::new(env.electrsd.electrum_url.as_str())?;
    let client = BdkElectrumClient::new(electrum_client);

    let spk_to_track = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
    let addr_to_track = Address::from_script(&spk_to_track, bdk_chain::bitcoin::Network::Regtest)?;

    // Setup receiver.
    let (mut recv_chain, _) = LocalChain::from_genesis_hash(env.bitcoind.client.get_block_hash(0)?);
    let mut recv_graph = IndexedTxGraph::<ConfirmationBlockTime, _>::new({
        let mut recv_index = SpkTxOutIndex::default();
        recv_index.insert_spk((), spk_to_track.clone());
        recv_index
    });

    // Mine some blocks.
    env.mine_blocks(101, None)?;

    // Send a preliminary tx such that the new utxo in Core's wallet
    // becomes the input of the next tx
    let new_addr = env
        .rpc_client()
        .get_new_address(None, None)?
        .assume_checked();
    let prev_amt = SEND_AMOUNT + FEE_AMOUNT;
    env.send(&new_addr, prev_amt)?;
    let prev_block_hash = env.mine_blocks(1, None)?.into_iter().next();

    let txid = env.send(&addr_to_track, SEND_AMOUNT)?;

    // Mine a block to confirm sent tx.
    let block_hash = env.mine_blocks(1, None)?.into_iter().next();

    // Look at the tx we just sent, it should have 1 input and 1 output
    let tx = env
        .rpc_client()
        .get_raw_transaction_info(&txid, block_hash.as_ref())?;
    assert_eq!(tx.vin.len(), 1);
    assert_eq!(tx.vout.len(), 1);
    let vin = &tx.vin[0];
    let prev_txid = vin.txid.unwrap();
    let vout = vin.vout.unwrap();
    let outpoint = bdk_chain::bitcoin::OutPoint::new(prev_txid, vout);

    // Get the txout of the previous tx
    let prev_tx = env
        .rpc_client()
        .get_raw_transaction_info(&prev_txid, prev_block_hash.as_ref())?;
    let txout = prev_tx
        .vout
        .iter()
        .find(|txout| txout.value == prev_amt)
        .unwrap();
    let script_pubkey = ScriptBuf::from_bytes(txout.script_pub_key.hex.to_vec());
    let txout = bdk_chain::bitcoin::TxOut {
        value: txout.value,
        script_pubkey,
    };

    // Sync up to tip.
    env.wait_until_electrum_sees_block(Duration::from_secs(6))?;
    let _ = sync_with_electrum(
        &client,
        [spk_to_track.clone()],
        &mut recv_chain,
        &mut recv_graph,
    )?;

    // Check the graph update contains the right floating txout
    let graph_txout = recv_graph
        .graph()
        .all_txouts()
        .find(|(_op, txout)| txout.value == prev_amt)
        .unwrap();
    assert_eq!(graph_txout, (outpoint, &txout));

    // Check to see if tx is confirmed.
    assert_eq!(
        get_balance(&recv_chain, &recv_graph)?,
        Balance {
            confirmed: SEND_AMOUNT,
            ..Balance::default()
        },
    );

    for tx in recv_graph.graph().full_txs() {
        // Retrieve the calculated fee from `TxGraph`, which will panic if we do not have the
        // floating txouts available from the transaction's previous outputs.
        let fee = recv_graph
            .graph()
            .calculate_fee(&tx.tx)
            .expect("fee must exist");

        // Check the fee calculated fee matches the initial fee amount
        assert_eq!(fee, FEE_AMOUNT);

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
        assert_eq!(fee, Amount::from_sat(tx_fee)); // 1650sat
    }
    Ok(())
}
