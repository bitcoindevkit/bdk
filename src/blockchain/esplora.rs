// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Esplora
//!
//! This module defines a [`Blockchain`] struct that can query an Esplora backend
//! populate the wallet's [database](crate::database::Database) by
//!
//! ## Example
//!
//! ```no_run
//! # use bdk::blockchain::esplora::EsploraBlockchain;
//! let blockchain = EsploraBlockchain::new("https://blockstream.info/testnet/api", None, 20);
//! # Ok::<(), bdk::Error>(())
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt;

use bitcoin::consensus::{self, deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{BlockHash, BlockHeader, Script, Transaction, Txid};
use futures::stream::{self, FuturesOrdered, StreamExt, TryStreamExt};
#[allow(unused_imports)]
use log::{debug, error, info, trace};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

use crate::database::BatchDatabase;
use crate::error::Error;
use crate::wallet::utils::ChunksIterator;
use crate::FeeRate;

use super::*;

use self::utils::{ElectrumLikeSync, ElsGetHistoryRes};

const DEFAULT_CONCURRENT_REQUESTS: u8 = 4;

#[derive(Debug)]
struct UrlClient {
    url: String,
    // We use the async client instead of the blocking one because it automatically uses `fetch`
    // when the target platform is wasm32.
    client: Client,
    concurrency: u8,
}

/// Structure that implements the logic to sync with Esplora
///
/// ## Example
/// See the [`blockchain::esplora`](crate::blockchain::esplora) module for a usage example.
#[derive(Debug)]
pub struct EsploraBlockchain {
    url_client: UrlClient,
    stop_gap: usize,
}

impl std::convert::From<UrlClient> for EsploraBlockchain {
    fn from(url_client: UrlClient) -> Self {
        EsploraBlockchain {
            url_client,
            stop_gap: 20,
        }
    }
}

impl EsploraBlockchain {
    /// Create a new instance of the client from a base URL
    pub fn new(base_url: &str, concurrency: Option<u8>, stop_gap: usize) -> Self {
        EsploraBlockchain {
            url_client: UrlClient {
                url: base_url.to_string(),
                client: Client::new(),
                concurrency: concurrency.unwrap_or(DEFAULT_CONCURRENT_REQUESTS),
            },
            stop_gap,
        }
    }
}

#[maybe_async]
impl Blockchain for EsploraBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![
            Capability::FullHistory,
            Capability::GetAnyTx,
            Capability::AccurateFees,
        ]
        .into_iter()
        .collect()
    }

    fn setup<D: BatchDatabase, P: Progress>(
        &self,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self
            .url_client
            .electrum_like_setup(self.stop_gap, database, progress_update))
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(await_or_block!(self.url_client._get_tx(txid))?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(await_or_block!(self.url_client._broadcast(tx))?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        Ok(await_or_block!(self.url_client._get_height())?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        let estimates = await_or_block!(self.url_client._get_fee_estimates())?;

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
            .get(&format!("{}/tx/{}/raw", self.url, txid))
            .send()
            .await?;

        if let StatusCode::NOT_FOUND = resp.status() {
            return Ok(None);
        }

        Ok(Some(deserialize(&resp.error_for_status()?.bytes().await?)?))
    }

    async fn _get_tx_no_opt(&self, txid: &Txid) -> Result<Transaction, EsploraError> {
        match self._get_tx(txid).await {
            Ok(Some(tx)) => Ok(tx),
            Ok(None) => Err(EsploraError::TransactionNotFound(*txid)),
            Err(e) => Err(e),
        }
    }

    async fn _get_header(&self, block_height: u32) -> Result<BlockHeader, EsploraError> {
        let resp = self
            .client
            .get(&format!("{}/block-height/{}", self.url, block_height))
            .send()
            .await?;

        if let StatusCode::NOT_FOUND = resp.status() {
            return Err(EsploraError::HeaderHeightNotFound(block_height));
        }
        let bytes = resp.bytes().await?;
        let hash = std::str::from_utf8(&bytes)
            .map_err(|_| EsploraError::HeaderHeightNotFound(block_height))?;

        let resp = self
            .client
            .get(&format!("{}/block/{}/header", self.url, hash))
            .send()
            .await?;

        let header = deserialize(&Vec::from_hex(&resp.text().await?)?)?;

        Ok(header)
    }

    async fn _broadcast(&self, transaction: &Transaction) -> Result<(), EsploraError> {
        self.client
            .post(&format!("{}/tx", self.url))
            .body(serialize(transaction).to_hex())
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    async fn _get_height(&self) -> Result<u32, EsploraError> {
        let req = self
            .client
            .get(&format!("{}/blocks/tip/height", self.url))
            .send()
            .await?;

        Ok(req.error_for_status()?.text().await?.parse()?)
    }

    async fn _script_get_history(
        &self,
        script: &Script,
    ) -> Result<Vec<ElsGetHistoryRes>, EsploraError> {
        let mut result = Vec::new();
        let scripthash = Self::script_to_scripthash(script);

        // Add the unconfirmed transactions first
        result.extend(
            self.client
                .get(&format!(
                    "{}/scripthash/{}/txs/mempool",
                    self.url, scripthash
                ))
                .send()
                .await?
                .error_for_status()?
                .json::<Vec<EsploraGetHistory>>()
                .await?
                .into_iter()
                .map(|x| ElsGetHistoryRes {
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
                    "{}/scripthash/{}/txs/chain/{}",
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

            result.extend(response.into_iter().map(|x| ElsGetHistoryRes {
                tx_hash: x.txid,
                height: x.status.block_height.unwrap_or(0) as i32,
            }));

            if len < 25 {
                break;
            }
        }

        Ok(result)
    }

    async fn _get_fee_estimates(&self) -> Result<HashMap<String, f64>, EsploraError> {
        Ok(self
            .client
            .get(&format!("{}/fee-estimates", self.url,))
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
    ) -> Result<Vec<Vec<ElsGetHistoryRes>>, Error> {
        let future = async {
            let mut results = vec![];
            for chunk in ChunksIterator::new(scripts.into_iter(), self.concurrency as usize) {
                let mut futs = FuturesOrdered::new();
                for script in chunk {
                    futs.push(self._script_get_history(&script));
                }
                let partial_results: Vec<Vec<ElsGetHistoryRes>> = futs.try_collect().await?;
                results.extend(partial_results);
            }
            Ok(stream::iter(results).collect().await)
        };

        await_or_block!(future)
    }

    fn els_batch_transaction_get<'s, I: IntoIterator<Item = &'s Txid>>(
        &self,
        txids: I,
    ) -> Result<Vec<Transaction>, Error> {
        let future = async {
            let mut results = vec![];
            for chunk in ChunksIterator::new(txids.into_iter(), self.concurrency as usize) {
                let mut futs = FuturesOrdered::new();
                for txid in chunk {
                    futs.push(self._get_tx_no_opt(&txid));
                }
                let partial_results: Vec<Transaction> = futs.try_collect().await?;
                results.extend(partial_results);
            }
            Ok(stream::iter(results).collect().await)
        };

        await_or_block!(future)
    }

    fn els_batch_block_header<I: IntoIterator<Item = u32>>(
        &self,
        heights: I,
    ) -> Result<Vec<BlockHeader>, Error> {
        let future = async {
            let mut results = vec![];
            for chunk in ChunksIterator::new(heights.into_iter(), self.concurrency as usize) {
                let mut futs = FuturesOrdered::new();
                for height in chunk {
                    futs.push(self._get_header(height));
                }
                let partial_results: Vec<BlockHeader> = futs.try_collect().await?;
                results.extend(partial_results);
            }
            Ok(stream::iter(results).collect().await)
        };

        await_or_block!(future)
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

/// Configuration for an [`EsploraBlockchain`]
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct EsploraBlockchainConfig {
    /// Base URL of the esplora service
    ///
    /// eg. `https://blockstream.info/api/`
    pub base_url: String,
    /// Number of parallel requests sent to the esplora service (default: 4)
    pub concurrency: Option<u8>,
    /// Stop searching addresses for transactions after finding an unused gap of this length
    pub stop_gap: usize,
}

impl ConfigurableBlockchain for EsploraBlockchain {
    type Config = EsploraBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        Ok(EsploraBlockchain::new(
            config.base_url.as_str(),
            config.concurrency,
            config.stop_gap,
        ))
    }
}

/// Errors that can happen during a sync with [`EsploraBlockchain`]
#[derive(Debug)]
pub enum EsploraError {
    /// Error with the HTTP call
    Reqwest(reqwest::Error),
    /// Invalid number returned
    Parsing(std::num::ParseIntError),
    /// Invalid Bitcoin data returned
    BitcoinEncoding(bitcoin::consensus::encode::Error),
    /// Invalid Hex data returned
    Hex(bitcoin::hashes::hex::Error),

    /// Transaction not found
    TransactionNotFound(Txid),
    /// Header height not found
    HeaderHeightNotFound(u32),
    /// Header hash not found
    HeaderHashNotFound(BlockHash),
}

impl fmt::Display for EsploraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for EsploraError {}

impl_error!(reqwest::Error, Reqwest, EsploraError);
impl_error!(std::num::ParseIntError, Parsing, EsploraError);
impl_error!(consensus::encode::Error, BitcoinEncoding, EsploraError);
impl_error!(bitcoin::hashes::hex::Error, Hex, EsploraError);

#[cfg(test)]
#[cfg(feature = "test-esplora")]
crate::bdk_blockchain_tests! {
    fn test_instance(test_client: &TestClient) -> EsploraBlockchain {
        EsploraBlockchain::new(&format!("http://{}",test_client.electrsd.esplora_url.as_ref().unwrap()), None, 20)
    }
}
