use bdk_chain::bitcoin::{Address, Amount};
use bdk_chain::local_chain::LocalChain;
use bdk_chain::spk_client::{FullScanRequest, SyncRequest};
use bdk_chain::spk_txout::SpkTxOutIndex;
use bdk_chain::{ConfirmationBlockTime, IndexedTxGraph, TxGraph};
use bdk_esplora::EsploraExt;
use bdk_testenv::corepc_node::{Input, Output};
use bdk_testenv::{anyhow, TestEnv};
use esplora_client::{self, Builder};
use std::collections::{BTreeSet, HashSet};
use std::str::FromStr;
use std::thread::sleep;
use std::time::Duration;

mod common;

// Ensure that a wallet can detect a malicious replacement of an incoming transaction.
//
// This checks that both the Esplora chain source and the receiving structures properly track the
// replaced transaction as missing.
#[test]
pub fn detect_receive_tx_cancel() -> anyhow::Result<()> {
    const SEND_TX_FEE: Amount = Amount::from_sat(1000);
    const UNDO_SEND_TX_FEE: Amount = Amount::from_sat(2000);

    let env = TestEnv::new()?;
    let rpc_client = env.rpc_client();
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_blocking();

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, _>::new(SpkTxOutIndex::<()>::default());
    let (chain, _) = LocalChain::from_genesis(env.genesis_hash()?);

    // Get receiving address.
    let receiver_spk = common::get_test_spk();
    let receiver_addr = Address::from_script(&receiver_spk, bdk_chain::bitcoin::Network::Regtest)?;
    graph.index.insert_spk((), receiver_spk);

    env.mine_blocks(101, None)?;

    // Select a UTXO to use as an input for constructing our test transactions.
    let selected_utxo = rpc_client
        .list_unspent()?
        .0
        .into_iter()
        // Find a block reward tx.
        .find(|utxo| utxo.amount == Amount::from_int_btc(50).to_btc())
        .expect("Must find a block reward UTXO")
        .into_model()?;

    // Derive the sender's address from the selected UTXO.
    let sender_spk = selected_utxo.script_pubkey.clone();
    let sender_addr = Address::from_script(&sender_spk, bdk_chain::bitcoin::Network::Regtest)
        .expect("Failed to derive address from UTXO");

    // Setup the common inputs used by both `send_tx` and `undo_send_tx`.
    let inputs = [Input {
        txid: selected_utxo.txid,
        vout: selected_utxo.vout as u64,
        sequence: None,
    }];

    // Create and sign the `send_tx` that sends funds to the receiver address.
    let send_tx_outputs = [Output::new(
        receiver_addr,
        selected_utxo.amount.to_unsigned()? - SEND_TX_FEE,
    )];

    let send_tx = rpc_client
        .create_raw_transaction(&inputs, &send_tx_outputs)?
        .into_model()?
        .0;
    let send_tx = rpc_client
        .sign_raw_transaction_with_wallet(&send_tx)?
        .into_model()?
        .tx;

    // Create and sign the `undo_send_tx` transaction. This redirects funds back to the sender
    // address.
    let undo_send_outputs = [Output::new(
        sender_addr,
        selected_utxo.amount.to_unsigned()? - UNDO_SEND_TX_FEE,
    )];
    let undo_send_tx = rpc_client
        .create_raw_transaction(&inputs, &undo_send_outputs)?
        .into_model()?
        .0;
    let undo_send_tx = rpc_client
        .sign_raw_transaction_with_wallet(&undo_send_tx)?
        .into_model()?
        .tx;

    // Sync after broadcasting the `send_tx`. Ensure that we detect and receive the `send_tx`.
    let send_txid = env.rpc_client().send_raw_transaction(&send_tx)?.txid()?;
    env.wait_until_electrum_sees_txid(send_txid, Duration::from_secs(6))?;
    let sync_request = SyncRequest::builder()
        .chain_tip(chain.tip())
        .spks_with_indexes(graph.index.all_spks().clone())
        .expected_spk_txids(
            graph
                .canonical_view(&chain, chain.tip().block_id(), Default::default())
                .list_expected_spk_txids(&graph.index, ..),
        );
    let sync_response = client.sync(sync_request, 1)?;
    assert!(
        sync_response
            .tx_update
            .txs
            .iter()
            .any(|tx| tx.compute_txid() == send_txid),
        "sync response must include the send_tx"
    );
    let changeset = graph.apply_update(sync_response.tx_update.clone());
    assert!(
        changeset.tx_graph.txs.contains(&send_tx),
        "tx graph must deem send_tx relevant and include it"
    );

    // Sync after broadcasting the `undo_send_tx`. Verify that `send_tx` is now missing from the
    // mempool.
    let undo_send_txid = env
        .rpc_client()
        .send_raw_transaction(&undo_send_tx)?
        .txid()?;
    env.wait_until_electrum_sees_txid(undo_send_txid, Duration::from_secs(6))?;
    let sync_request = SyncRequest::builder()
        .chain_tip(chain.tip())
        .spks_with_indexes(graph.index.all_spks().clone())
        .expected_spk_txids(
            graph
                .canonical_view(&chain, chain.tip().block_id(), Default::default())
                .list_expected_spk_txids(&graph.index, ..),
        );
    let sync_response = client.sync(sync_request, 1)?;
    assert!(
        sync_response
            .tx_update
            .evicted_ats
            .iter()
            .any(|(txid, _)| *txid == send_txid),
        "sync response must track send_tx as missing from mempool"
    );
    let changeset = graph.apply_update(sync_response.tx_update.clone());
    assert!(
        changeset.tx_graph.last_evicted.contains_key(&send_txid),
        "tx graph must track send_tx as missing"
    );

    Ok(())
}

#[test]
pub fn test_update_tx_graph_without_keychain() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_blocking();

    let receive_address0 =
        Address::from_str("bcrt1qc6fweuf4xjvz4x3gx3t9e0fh4hvqyu2qw4wvxm")?.assume_checked();
    let receive_address1 =
        Address::from_str("bcrt1qfjg5lv3dvc9az8patec8fjddrs4aqtauadnagr")?.assume_checked();

    let misc_spks = [
        receive_address0.script_pubkey(),
        receive_address1.script_pubkey(),
    ];

    let _block_hashes = env.mine_blocks(101, None)?;
    let txid1 = env
        .bitcoind
        .client
        .send_to_address(&receive_address1, Amount::from_sat(10000))?
        .txid()?;
    let txid2 = env
        .bitcoind
        .client
        .send_to_address(&receive_address0, Amount::from_sat(20000))?
        .txid()?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while client.get_height().unwrap() < 102 {
        sleep(Duration::from_millis(10))
    }

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    let sync_update = {
        let request = SyncRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks(misc_spks);
        client.sync(request, 1)?
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
            .get_transaction(tx.compute_txid())
            .expect("Tx must exist")
            .fee
            .expect("Fee must exist")
            .abs();

        // Check that the calculated fee matches the fee from the transaction data.
        assert_eq!(
            fee,
            Amount::from_float_in(tx_fee, bdk_core::bitcoin::Denomination::Bitcoin)?
        );
    }

    assert_eq!(
        tx_update
            .txs
            .iter()
            .map(|tx| tx.compute_txid())
            .collect::<BTreeSet<_>>(),
        [txid1, txid2].into(),
        "update must include all expected transactions"
    );
    Ok(())
}

/// Test the bounds of the address scan depending on the `stop_gap`.
#[test]
pub fn test_update_tx_graph_stop_gap() -> anyhow::Result<()> {
    let env = TestEnv::new()?;
    let base_url = format!("http://{}", &env.electrsd.esplora_url.clone().unwrap());
    let client = Builder::new(base_url.as_str()).build_blocking();
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
    let txid_4th_addr = env
        .bitcoind
        .client
        .send_to_address(&addresses[3], Amount::from_sat(10000))?
        .into_model()?
        .txid;
    let _block_hashes = env.mine_blocks(1, None)?;
    while client.get_height().unwrap() < 103 {
        sleep(Duration::from_millis(10))
    }

    // use a full checkpoint linked list (since this is not what we are testing)
    let cp_tip = env.make_checkpoint_tip();

    // A scan with a stop_gap of 3 won't find the transaction, but a scan with a gap limit of 4
    // will.
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 3, 1)?
    };
    assert!(full_scan_update.tx_update.txs.is_empty());
    assert!(full_scan_update.last_active_indices.is_empty());
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 4, 1)?
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
    let txid_last_addr = env
        .bitcoind
        .client
        .send_to_address(&addresses[addresses.len() - 1], Amount::from_sat(10000))?
        .txid()?;
    let _block_hashes = env.mine_blocks(1, None)?;
    while client.get_height().unwrap() < 104 {
        sleep(Duration::from_millis(10))
    }

    // A scan with gap limit 5 won't find the second transaction, but a scan with gap limit 6 will.
    // The last active indice won't be updated in the first case but will in the second one.
    let full_scan_update = {
        let request = FullScanRequest::builder()
            .chain_tip(cp_tip.clone())
            .spks_for_keychain(0, spks.clone());
        client.full_scan(request, 5, 1)?
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
        client.full_scan(request, 6, 1)?
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
