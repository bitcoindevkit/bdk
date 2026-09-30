use std::borrow::Borrow;

use bdk_chain::bitcoin::{
    absolute, consensus, hashes::Hash, transaction, Amount, Script, ScriptBuf, Transaction, TxIn,
    TxOut, Txid, WPubkeyHash,
};
use bdk_chain::spk_client::SyncRequest;
use bdk_electrum::electrum_client::{
    Batch, ElectrumApi, Error, GetBalanceRes, GetHeadersRes, GetHistoryRes, GetMerkleRes,
    ListUnspentRes, Param, RawHeaderNotification, ScriptStatus, ServerFeaturesRes, TxidFromPosRes,
};
use bdk_electrum::BdkElectrumClient;

/// Transport that answers every batch request, but returns one entry fewer than requested for
/// each batch method whose flag is set.
#[derive(Default)]
struct ShortBatchTransport {
    txs: Vec<(Transaction, usize)>,
    short_headers: bool,
    short_history: bool,
    short_merkle: bool,
}

impl ElectrumApi for ShortBatchTransport {
    fn batch_script_get_history<'s, I>(&self, scripts: I) -> Result<Vec<Vec<GetHistoryRes>>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<&'s Script>,
    {
        let history = self
            .txs
            .iter()
            .map(|(tx, height)| GetHistoryRes {
                height: *height as i32,
                tx_hash: tx.compute_txid(),
                fee: None,
            })
            .collect::<Vec<_>>();
        let mut out: Vec<_> = scripts.into_iter().map(|_| history.clone()).collect();
        if self.short_history {
            out.pop();
        }
        Ok(out)
    }

    fn transaction_get_raw(&self, txid: &Txid) -> Result<Vec<u8>, Error> {
        let (tx, _) = self
            .txs
            .iter()
            .find(|(tx, _)| tx.compute_txid() == *txid)
            .unwrap();
        Ok(consensus::serialize(tx))
    }

    fn batch_block_header_raw<I>(&self, heights: I) -> Result<Vec<Vec<u8>>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<u32>,
    {
        let requested = heights.into_iter().count();
        let len = requested - usize::from(self.short_headers);
        Ok(vec![vec![0u8; 80]; len])
    }

    fn block_header_raw(&self, _: usize) -> Result<Vec<u8>, Error> {
        Ok(vec![0u8; 80])
    }

    fn batch_transaction_get_merkle<I>(
        &self,
        txids_and_heights: I,
    ) -> Result<Vec<GetMerkleRes>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<(Txid, usize)>,
    {
        let mut out = txids_and_heights
            .into_iter()
            .map(|item| GetMerkleRes {
                block_height: item.borrow().1,
                pos: 0,
                merkle: vec![],
            })
            .collect::<Vec<_>>();
        if self.short_merkle {
            out.pop();
        }
        Ok(out)
    }

    // Not exercised by `sync`.
    fn raw_call(
        &self,
        _: &str,
        _: impl IntoIterator<Item = Param>,
    ) -> Result<serde_json::Value, Error> {
        unimplemented!()
    }
    fn batch_call(&self, _: &Batch) -> Result<Vec<serde_json::Value>, Error> {
        unimplemented!()
    }
    fn block_headers_subscribe_raw(&self) -> Result<RawHeaderNotification, Error> {
        unimplemented!()
    }
    fn block_headers_pop_raw(&self) -> Result<Option<RawHeaderNotification>, Error> {
        unimplemented!()
    }
    fn block_headers(&self, _: usize, _: usize) -> Result<GetHeadersRes, Error> {
        unimplemented!()
    }
    fn estimate_fee(&self, _: usize) -> Result<f64, Error> {
        unimplemented!()
    }
    fn relay_fee(&self) -> Result<f64, Error> {
        unimplemented!()
    }
    fn script_subscribe(&self, _: &Script) -> Result<Option<ScriptStatus>, Error> {
        unimplemented!()
    }
    fn batch_script_subscribe<'s, I>(&self, _: I) -> Result<Vec<Option<ScriptStatus>>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<&'s Script>,
    {
        unimplemented!()
    }
    fn script_unsubscribe(&self, _: &Script) -> Result<bool, Error> {
        unimplemented!()
    }
    fn script_pop(&self, _: &Script) -> Result<Option<ScriptStatus>, Error> {
        unimplemented!()
    }
    fn script_get_balance(&self, _: &Script) -> Result<GetBalanceRes, Error> {
        unimplemented!()
    }
    fn batch_script_get_balance<'s, I>(&self, _: I) -> Result<Vec<GetBalanceRes>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<&'s Script>,
    {
        unimplemented!()
    }
    fn script_get_history(&self, _: &Script) -> Result<Vec<GetHistoryRes>, Error> {
        unimplemented!()
    }
    fn script_list_unspent(&self, _: &Script) -> Result<Vec<ListUnspentRes>, Error> {
        unimplemented!()
    }
    fn batch_script_list_unspent<'s, I>(&self, _: I) -> Result<Vec<Vec<ListUnspentRes>>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<&'s Script>,
    {
        unimplemented!()
    }
    fn batch_transaction_get_raw<'t, I>(&self, _: I) -> Result<Vec<Vec<u8>>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<&'t Txid>,
    {
        unimplemented!()
    }
    fn batch_estimate_fee<I>(&self, _: I) -> Result<Vec<f64>, Error>
    where
        I: IntoIterator + Clone,
        I::Item: Borrow<usize>,
    {
        unimplemented!()
    }
    fn transaction_broadcast_raw(&self, _: &[u8]) -> Result<Txid, Error> {
        unimplemented!()
    }
    fn transaction_get_merkle(&self, _: &Txid, _: usize) -> Result<GetMerkleRes, Error> {
        unimplemented!()
    }
    fn txid_from_pos(&self, _: usize, _: usize) -> Result<Txid, Error> {
        unimplemented!()
    }
    fn txid_from_pos_with_merkle(&self, _: usize, _: usize) -> Result<TxidFromPosRes, Error> {
        unimplemented!()
    }
    fn server_features(&self) -> Result<ServerFeaturesRes, Error> {
        unimplemented!()
    }
    fn ping(&self) -> Result<(), Error> {
        unimplemented!()
    }
}

fn test_spk(byte: u8) -> ScriptBuf {
    ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([byte; 20]))
}

fn tx_paying(spk: &ScriptBuf, lock_time: u32) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::from_consensus(lock_time),
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: spk.clone(),
        }],
    }
}

/// Two transactions confirmed at different heights, so that two headers and two merkle proofs
/// are requested in a single batch.
fn two_confirmed_txs(spk: &ScriptBuf) -> Vec<(Transaction, usize)> {
    vec![(tx_paying(spk, 1), 10), (tx_paying(spk, 2), 20)]
}

#[test]
fn short_header_batch_is_reported_as_an_error() {
    let spk = test_spk(1);
    let client = BdkElectrumClient::new(ShortBatchTransport {
        txs: two_confirmed_txs(&spk),
        short_headers: true,
        ..Default::default()
    });

    let result = client.sync(SyncRequest::builder().spks([spk]), 10, false);
    assert!(result.is_err(), "sync accepted a short header batch");
}

#[test]
fn short_merkle_batch_is_reported_as_an_error() {
    let spk = test_spk(1);
    let client = BdkElectrumClient::new(ShortBatchTransport {
        txs: two_confirmed_txs(&spk),
        short_merkle: true,
        ..Default::default()
    });

    let result = client.sync(SyncRequest::builder().spks([spk]), 10, false);
    assert!(result.is_err(), "sync accepted a short merkle proof batch");
}

#[test]
fn short_history_batch_is_reported_as_an_error() {
    let spk_a = test_spk(1);
    let spk_b = test_spk(2);
    let expected_txid = Txid::from_byte_array([9; 32]);
    let client = BdkElectrumClient::new(ShortBatchTransport {
        txs: vec![(tx_paying(&spk_a, 1), 0)],
        short_history: true,
        ..Default::default()
    });

    let request = SyncRequest::builder()
        .spks([spk_a, spk_b.clone()])
        .expected_spk_txids([(spk_b, expected_txid)]);
    let result = client.sync(request, 10, false);
    assert!(result.is_err(), "sync accepted a short history batch");
}
