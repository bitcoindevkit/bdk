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

//! Esplora by way of `reqwest` HTTP client.

use std::collections::{HashMap, HashSet};

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{BlockHeader, Script, Transaction, Txid};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use ::reqwest::{Client, StatusCode};
use futures::stream::{FuturesOrdered, TryStreamExt};

use super::api::Tx;
use crate::blockchain::esplora::EsploraError;
use crate::blockchain::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

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
    /// Create a new instance of the client from a base URL and `stop_gap`.
    pub fn new(base_url: &str, stop_gap: usize) -> Self {
        EsploraBlockchain {
            url_client: UrlClient {
                url: base_url.to_string(),
                client: Client::new(),
                concurrency: super::DEFAULT_CONCURRENT_REQUESTS,
            },
            stop_gap,
        }
    }

    /// Set the concurrency to use when doing batch queries against the Esplora instance.
    pub fn with_concurrency(mut self, concurrency: u8) -> Self {
        self.url_client.concurrency = concurrency;
        self
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
        _progress_update: P,
    ) -> Result<(), Error> {
        use crate::blockchain::script_sync::Request;
        let mut request = script_sync::start(database, self.stop_gap)?;
        let mut tx_index: HashMap<Txid, Tx> = HashMap::new();

        let batch_update = loop {
            request = match request {
                Request::Script(script_req) => {
                    let futures: FuturesOrdered<_> = script_req
                        .request()
                        .take(self.url_client.concurrency as usize)
                        .map(|script| async move {
                            let mut related_txs: Vec<Tx> =
                                self.url_client._scripthash_txs(script, None).await?;

                            let n_confirmed =
                                related_txs.iter().filter(|tx| tx.status.confirmed).count();
                            // esplora pages on 25 confirmed transactions. If there's 25 or more we
                            // keep requesting to see if there's more.
                            if n_confirmed >= 25 {
                                loop {
                                    let new_related_txs: Vec<Tx> = self
                                        .url_client
                                        ._scripthash_txs(
                                            script,
                                            Some(related_txs.last().unwrap().txid),
                                        )
                                        .await?;
                                    let n = new_related_txs.len();
                                    related_txs.extend(new_related_txs);
                                    // we've reached the end
                                    if n < 25 {
                                        break;
                                    }
                                }
                            }
                            Result::<_, Error>::Ok(related_txs)
                        })
                        .collect();
                    let txs_per_script: Vec<Vec<Tx>> = await_or_block!(futures.try_collect())?;
                    let mut satisfaction = vec![];

                    for txs in txs_per_script {
                        satisfaction.push(
                            txs.iter()
                                .map(|tx| (tx.txid, tx.status.block_height))
                                .collect(),
                        );
                        for tx in txs {
                            tx_index.insert(tx.txid, tx);
                        }
                    }

                    script_req.satisfy(satisfaction)?
                }
                Request::Conftime(conftime_req) => {
                    let conftimes = conftime_req
                        .request()
                        .map(|txid| {
                            tx_index
                                .get(txid)
                                .expect("must be in index")
                                .confirmation_time()
                        })
                        .collect();
                    conftime_req.satisfy(conftimes)?
                }
                Request::Tx(tx_req) => {
                    let full_txs = tx_req
                        .request()
                        .map(|txid| {
                            let tx = tx_index.get(txid).expect("must be in index");
                            (tx.previous_outputs(), tx.to_tx())
                        })
                        .collect();
                    tx_req.satisfy(full_txs)?
                }
                Request::Finish(batch_update) => break batch_update,
            }
        };

        database.commit_batch(batch_update)?;

        Ok(())
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
        super::into_fee_rate(target, estimates)
    }
}

impl UrlClient {
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

    async fn _scripthash_txs(
        &self,
        script: &Script,
        last_seen: Option<Txid>,
    ) -> Result<Vec<Tx>, EsploraError> {
        let script_hash = sha256::Hash::hash(script.as_bytes()).into_inner().to_hex();
        let url = match last_seen {
            Some(last_seen) => format!(
                "{}/scripthash/{}/txs/chain/{}",
                self.url, script_hash, last_seen
            ),
            None => format!("{}/scripthash/{}/txs", self.url, script_hash),
        };
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<Tx>>()
            .await?)
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

impl ConfigurableBlockchain for EsploraBlockchain {
    type Config = super::EsploraBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let map_e = |e: reqwest::Error| Error::Esplora(Box::new(e.into()));

        let mut blockchain = EsploraBlockchain::new(config.base_url.as_str(), config.stop_gap);
        if let Some(concurrency) = config.concurrency {
            blockchain.url_client.concurrency = concurrency;
        }
        let mut builder = Client::builder();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(map_e)?);
        }

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(timeout) = config.timeout {
            builder = builder.timeout(core::time::Duration::from_secs(timeout));
        }

        blockchain.url_client.client = builder.build().map_err(map_e)?;

        Ok(blockchain)
    }
}
