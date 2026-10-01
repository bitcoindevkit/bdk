use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

use bdk_chain::bitcoin::{
    absolute, consensus, hashes::Hash, transaction, Amount, OutPoint, ScriptBuf, Transaction, TxIn,
    TxOut, Txid, WPubkeyHash,
};
use bdk_chain::spk_client::SyncRequest;
use bdk_electrum::electrum_client::{Client, ConfigBuilder};
use bdk_electrum::BdkElectrumClient;

/// Electrum stub that reports `tx` as confirmed at height 100 but serves a merkle proof that does
/// not match the block header it serves.
fn serve_confirmed_with_bad_proof(listener: TcpListener, tx: Transaction) {
    let (txid, raw_tx) = (tx.compute_txid(), consensus::encode::serialize_hex(&tx));
    let raw_header = "00".repeat(80);
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        for request in BufReader::new(stream.try_clone().unwrap()).lines() {
            let request = request.unwrap();
            let id_start = request.find("\"id\":").unwrap() + 5;
            let id: String = request[id_start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let result = if request.contains("blockchain.scripthash.get_history") {
                format!(r#"[{{"height":100,"tx_hash":"{txid}"}}]"#)
            } else if request.contains("blockchain.transaction.get_merkle") {
                r#"{"block_height":100,"pos":0,"merkle":[]}"#.to_string()
            } else if request.contains("blockchain.transaction.get") {
                format!(r#""{raw_tx}""#)
            } else if request.contains("blockchain.block.header") {
                format!(r#""{raw_header}""#)
            } else {
                panic!("unexpected request: {request}");
            };
            writeln!(stream, r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#).unwrap();
        }
    }
}

#[test]
fn confirmed_tx_with_failing_proof_keeps_temporal_context() {
    let wallet_spk = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([1; 20]));
    let payment = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        // Must not be a coinbase: coinbase txs never get a `seen_at`.
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([2; 32]),
                vout: 0,
            },
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: wallet_spk.clone(),
        }],
    };
    let txid = payment.compute_txid();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("tcp://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || serve_confirmed_with_bad_proof(listener, payment));

    let config = ConfigBuilder::new().retry(0).build();
    let client = BdkElectrumClient::new(Client::from_config(&url, config).unwrap());
    let response = client
        .sync(SyncRequest::builder().spks([wallet_spk]), 1, false)
        .unwrap();
    let update = response.tx_update;

    assert!(
        update.txs.iter().any(|tx| tx.compute_txid() == txid),
        "tx not in update"
    );
    let anchored = update.anchors.iter().any(|(_, t)| *t == txid);
    let seen = update.seen_ats.iter().any(|(t, _)| *t == txid);
    assert!(
        anchored || seen,
        "tx {txid} has neither an anchor nor a seen_at"
    );
}
