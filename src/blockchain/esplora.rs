use std::collections::{HashMap, HashSet};

use futures::stream::{self, StreamExt, TryStreamExt};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use serde::Deserialize;

use reqwest::{Client, StatusCode};

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::hex::ToHex;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{Script, Transaction, Txid};

use self::utils::{ELSGetHistoryRes, ELSListUnspentRes, ElectrumLikeSync};
use super::*;
use crate::database::{BatchDatabase, DatabaseUtils};
use crate::error::Error;
use crate::FeeRate;

#[derive(Debug)]
pub struct UrlClient {
    url: String,
    // We use the async client instead of the blocking one because it automatically uses `fetch`
    // when the target platform is wasm32.
    client: Client,
}

#[derive(Debug)]
pub struct EsploraBlockchain(Option<UrlClient>);

impl std::convert::From<UrlClient> for EsploraBlockchain {
    fn from(url_client: UrlClient) -> Self {
        EsploraBlockchain(Some(url_client))
    }
}

impl EsploraBlockchain {
    pub fn new(base_url: &str) -> Self {
        EsploraBlockchain(Some(UrlClient {
            url: base_url.to_string(),
            client: Client::new(),
        }))
    }
}

impl Blockchain for EsploraBlockchain {
    fn offline() -> Self {
        EsploraBlockchain(None)
    }

    fn is_online(&self) -> bool {
        self.0.is_some()
    }
}

#[maybe_async]
impl OnlineBlockchain for EsploraBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![Capability::FullHistory, Capability::GetAnyTx]
            .into_iter()
            .collect()
    }

    fn setup<D: BatchDatabase + DatabaseUtils, P: Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .electrum_like_setup(stop_gap, database, progress_update))
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(await_or_block!(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            ._get_tx(txid))?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(await_or_block!(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            ._broadcast(tx))?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        Ok(await_or_block!(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            ._get_height())?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        let estimates = await_or_block!(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            ._get_fee_estimates())?;

        let fee_val = estimates
            .into_iter()
            .map(|(k, v)| Ok::<_, std::num::ParseIntError>((k.parse::<usize>()?, v)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| Error::Generic(e.to_string()))?
            .into_iter()
            .take_while(|(k, _)| k <= &target)
            .map(|(_, v)| v)
            .last()
            .unwrap_or(1.0);

        Ok(FeeRate::from_sat_per_vb(fee_val as f32))
    }
}

impl UrlClient {
    fn script_to_scripthash(script: &Script) -> String {
        sha256::Hash::hash(script.as_bytes()).into_inner().to_hex()
    }

    async fn _get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, EsploraError> {
        let resp = self
            .client
            .get(&format!("{}/api/tx/{}/raw", self.url, txid))
            .send()
            .await?;

        if let StatusCode::NOT_FOUND = resp.status() {
            return Ok(None);
        }

        Ok(Some(deserialize(&resp.error_for_status()?.bytes().await?)?))
    }

    async fn _broadcast(&self, transaction: &Transaction) -> Result<(), EsploraError> {
        self.client
            .post(&format!("{}/api/tx", self.url))
            .body(serialize(transaction).to_hex())
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn _get_height(&self) -> Result<u32, EsploraError> {
        let req = self
            .client
            .get(&format!("{}/api/blocks/tip/height", self.url))
            .send()
            .await?;

        Ok(req.error_for_status()?.text().await?.parse()?)
    }

    async fn _script_get_history(
        &self,
        script: &Script,
    ) -> Result<Vec<ELSGetHistoryRes>, EsploraError> {
        let mut result = Vec::new();
        let scripthash = Self::script_to_scripthash(script);

        // Add the unconfirmed transactions first
        result.extend(
            self.client
                .get(&format!(
                    "{}/api/scripthash/{}/txs/mempool",
                    self.url, scripthash
                ))
                .send()
                .await?
                .error_for_status()?
                .json::<Vec<EsploraGetHistory>>()
                .await?
                .into_iter()
                .map(|x| ELSGetHistoryRes {
                    tx_hash: x.txid,
                    height: x.status.block_height.unwrap_or(0) as i32,
                }),
        );

        debug!(
            "Found {} mempool txs for {} - {:?}",
            result.len(),
            scripthash,
            script
        );

        // Then go through all the pages of confirmed transactions
        let mut last_txid = String::new();
        loop {
            let response = self
                .client
                .get(&format!(
                    "{}/api/scripthash/{}/txs/chain/{}",
                    self.url, scripthash, last_txid
                ))
                .send()
                .await?
                .error_for_status()?
                .json::<Vec<EsploraGetHistory>>()
                .await?;
            let len = response.len();
            if let Some(elem) = response.last() {
                last_txid = elem.txid.to_hex();
            }

            debug!("... adding {} confirmed transactions", len);

            result.extend(response.into_iter().map(|x| ELSGetHistoryRes {
                tx_hash: x.txid,
                height: x.status.block_height.unwrap_or(0) as i32,
            }));

            if len < 25 {
                break;
            }
        }

        Ok(result)
    }

    async fn _script_list_unspent(
        &self,
        script: &Script,
    ) -> Result<Vec<ELSListUnspentRes>, EsploraError> {
        Ok(self
            .client
            .get(&format!(
                "{}/api/scripthash/{}/utxo",
                self.url,
                Self::script_to_scripthash(script)
            ))
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<EsploraListUnspent>>()
            .await?
            .into_iter()
            .map(|x| ELSListUnspentRes {
                tx_hash: x.txid,
                height: x.status.block_height.unwrap_or(0),
                tx_pos: x.vout,
            })
            .collect())
    }

    async fn _get_fee_estimates(&self) -> Result<HashMap<String, f64>, EsploraError> {
        Ok(self
            .client
            .get(&format!("{}/api/fee-estimates", self.url,))
            .send()
            .await?
            .error_for_status()?
            .json::<HashMap<String, f64>>()
            .await?)
    }
}

#[maybe_async]
impl ElectrumLikeSync for UrlClient {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSGetHistoryRes>>, Error> {
        let future = async {
            Ok(stream::iter(scripts)
                .then(|script| self._script_get_history(&script))
                .try_collect()
                .await?)
        };

        await_or_block!(future)
    }

    fn els_batch_script_list_unspent<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSListUnspentRes>>, Error> {
        let future = async {
            Ok(stream::iter(scripts)
                .then(|script| self._script_list_unspent(&script))
                .try_collect()
                .await?)
        };

        await_or_block!(future)
    }

    fn els_transaction_get(&self, txid: &Txid) -> Result<Transaction, Error> {
        Ok(await_or_block!(self._get_tx(txid))?
            .ok_or_else(|| EsploraError::TransactionNotFound(*txid))?)
    }
}

#[derive(Deserialize)]
struct EsploraGetHistoryStatus {
    block_height: Option<usize>,
}

#[derive(Deserialize)]
struct EsploraGetHistory {
    txid: Txid,
    status: EsploraGetHistoryStatus,
}

#[derive(Deserialize)]
struct EsploraListUnspent {
    txid: Txid,
    vout: usize,
    status: EsploraGetHistoryStatus,
}

#[derive(Debug)]
pub enum EsploraError {
    Reqwest(reqwest::Error),
    Parsing(std::num::ParseIntError),
    BitcoinEncoding(bitcoin::consensus::encode::Error),

    TransactionNotFound(Txid),
}

impl From<reqwest::Error> for EsploraError {
    fn from(other: reqwest::Error) -> Self {
        EsploraError::Reqwest(other)
    }
}

impl From<std::num::ParseIntError> for EsploraError {
    fn from(other: std::num::ParseIntError) -> Self {
        EsploraError::Parsing(other)
    }
}

impl From<bitcoin::consensus::encode::Error> for EsploraError {
    fn from(other: bitcoin::consensus::encode::Error) -> Self {
        EsploraError::BitcoinEncoding(other)
    }
}
