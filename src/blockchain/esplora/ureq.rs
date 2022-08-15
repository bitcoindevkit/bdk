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

//! Esplora by way of `ureq` HTTP client.

use std::collections::{HashMap, HashSet};
use std::io;
use std::io::Read;
use std::time::Duration;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use ureq::{Agent, Proxy, Response};

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{BlockHeader, Script, Transaction, Txid};

use super::api::Tx;
use crate::blockchain::esplora::EsploraError;
use crate::blockchain::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

/// Structure that encapsulates Esplora client
#[derive(Debug, Clone)]
pub struct UrlClient {
    url: String,
    agent: Agent,
}

/// Structure that implements the logic to sync with Esplora
///
/// ## Example
/// See the [`blockchain::esplora`](crate::blockchain::esplora) module for a usage example.
#[derive(Debug)]
pub struct EsploraBlockchain {
    url_client: UrlClient,
    stop_gap: usize,
    concurrency: u8,
}

impl EsploraBlockchain {
    /// Create a new instance of the client from a base URL and the `stop_gap`.
    pub fn new(base_url: &str, stop_gap: usize) -> Self {
        EsploraBlockchain {
            url_client: UrlClient {
                url: base_url.to_string(),
                agent: Agent::new(),
            },
            concurrency: super::DEFAULT_CONCURRENT_REQUESTS,
            stop_gap,
        }
    }

    /// Set the inner `ureq` agent.
    pub fn with_agent(mut self, agent: Agent) -> Self {
        self.url_client.agent = agent;
        self
    }

    /// Set the number of parallel requests the client can make.
    pub fn with_concurrency(mut self, concurrency: u8) -> Self {
        self.concurrency = concurrency;
        self
    }
}

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

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        self.url_client._broadcast(tx)?;
        Ok(())
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        let estimates = self.url_client._get_fee_estimates()?;
        super::into_fee_rate(target, estimates)
    }
}

impl Deref for EsploraBlockchain {
    type Target = UrlClient;

    fn deref(&self) -> &Self::Target {
        &self.url_client
    }
}

impl StatelessBlockchain for EsploraBlockchain {}

impl GetHeight for EsploraBlockchain {
    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.url_client._get_height()?)
    }
}

impl GetTx for EsploraBlockchain {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.url_client._get_tx(txid)?)
    }
}

impl GetBlockHash for EsploraBlockchain {
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error> {
        let block_header = self.url_client._get_header(height as u32)?;
        Ok(block_header.block_hash())
    }
}

impl WalletSync for EsploraBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        use crate::blockchain::script_sync::Request;
        let mut request = script_sync::start(database, self.stop_gap)?;
        let mut tx_index: HashMap<Txid, Tx> = HashMap::new();
        let batch_update = loop {
            request = match request {
                Request::Script(script_req) => {
                    let scripts = script_req
                        .request()
                        .take(self.concurrency as usize)
                        .cloned();

                    let mut handles = vec![];
                    for script in scripts {
                        let client = self.url_client.clone();
                        // make each request in its own thread.
                        handles.push(std::thread::spawn(move || {
                            let mut related_txs: Vec<Tx> = client._scripthash_txs(&script, None)?;

                            let n_confirmed =
                                related_txs.iter().filter(|tx| tx.status.confirmed).count();
                            // esplora pages on 25 confirmed transactions. If there's 25 or more we
                            // keep requesting to see if there's more.
                            if n_confirmed >= 25 {
                                loop {
                                    let new_related_txs: Vec<Tx> = client._scripthash_txs(
                                        &script,
                                        Some(related_txs.last().unwrap().txid),
                                    )?;
                                    let n = new_related_txs.len();
                                    related_txs.extend(new_related_txs);
                                    // we've reached the end
                                    if n < 25 {
                                        break;
                                    }
                                }
                            }
                            Result::<_, Error>::Ok(related_txs)
                        }));
                    }

                    let txs_per_script: Vec<Vec<Tx>> = handles
                        .into_iter()
                        .map(|handle| handle.join().unwrap())
                        .collect::<Result<_, _>>()?;
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
                            Ok((tx.previous_outputs(), tx.to_tx()))
                        })
                        .collect::<Result<_, Error>>()?;
                    tx_req.satisfy(full_txs)?
                }
                Request::Finish(batch_update) => break batch_update,
            }
        };

        database.commit_batch(batch_update)?;

        Ok(())
    }
}

impl UrlClient {
    fn _get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, EsploraError> {
        let resp = self
            .agent
            .get(&format!("{}/tx/{}/raw", self.url, txid))
            .call();

        match resp {
            Ok(resp) => Ok(Some(deserialize(&into_bytes(resp)?)?)),
            Err(ureq::Error::Status(code, _)) => {
                if is_status_not_found(code) {
                    return Ok(None);
                }
                Err(EsploraError::HttpResponse(code))
            }
            Err(e) => Err(EsploraError::Ureq(e)),
        }
    }

    fn _get_tx_no_opt(&self, txid: &Txid) -> Result<Transaction, EsploraError> {
        match self._get_tx(txid) {
            Ok(Some(tx)) => Ok(tx),
            Ok(None) => Err(EsploraError::TransactionNotFound(*txid)),
            Err(e) => Err(e),
        }
    }

    fn _get_header(&self, block_height: u32) -> Result<BlockHeader, EsploraError> {
        let resp = self
            .agent
            .get(&format!("{}/block-height/{}", self.url, block_height))
            .call();

        let bytes = match resp {
            Ok(resp) => Ok(into_bytes(resp)?),
            Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
            Err(e) => Err(EsploraError::Ureq(e)),
        }?;

        let hash = std::str::from_utf8(&bytes)
            .map_err(|_| EsploraError::HeaderHeightNotFound(block_height))?;

        let resp = self
            .agent
            .get(&format!("{}/block/{}/header", self.url, hash))
            .call();

        match resp {
            Ok(resp) => Ok(deserialize(&Vec::from_hex(&resp.into_string()?)?)?),
            Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
            Err(e) => Err(EsploraError::Ureq(e)),
        }
    }

    fn _broadcast(&self, transaction: &Transaction) -> Result<(), EsploraError> {
        let resp = self
            .agent
            .post(&format!("{}/tx", self.url))
            .send_string(&serialize(transaction).to_hex());

        match resp {
            Ok(_) => Ok(()), // We do not return the txid?
            Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
            Err(e) => Err(EsploraError::Ureq(e)),
        }
    }

    fn _get_height(&self) -> Result<u32, EsploraError> {
        let resp = self
            .agent
            .get(&format!("{}/blocks/tip/height", self.url))
            .call();

        match resp {
            Ok(resp) => Ok(resp.into_string()?.parse()?),
            Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
            Err(e) => Err(EsploraError::Ureq(e)),
        }
    }

    fn _get_fee_estimates(&self) -> Result<HashMap<String, f64>, EsploraError> {
        let resp = self
            .agent
            .get(&format!("{}/fee-estimates", self.url,))
            .call();

        let map = match resp {
            Ok(resp) => {
                let map: HashMap<String, f64> = resp.into_json()?;
                Ok(map)
            }
            Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
            Err(e) => Err(EsploraError::Ureq(e)),
        }?;

        Ok(map)
    }

    fn _scripthash_txs(
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
        Ok(self.agent.get(&url).call()?.into_json()?)
    }
}

fn is_status_not_found(status: u16) -> bool {
    status == 404
}

fn into_bytes(resp: Response) -> Result<Vec<u8>, io::Error> {
    const BYTES_LIMIT: usize = 10 * 1_024 * 1_024;

    let mut buf: Vec<u8> = vec![];
    resp.into_reader()
        .take((BYTES_LIMIT + 1) as u64)
        .read_to_end(&mut buf)?;
    if buf.len() > BYTES_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "response too big for into_bytes",
        ));
    }

    Ok(buf)
}

impl ConfigurableBlockchain for EsploraBlockchain {
    type Config = super::EsploraBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let mut agent_builder = ureq::AgentBuilder::new();

        if let Some(timeout) = config.timeout {
            agent_builder = agent_builder.timeout(Duration::from_secs(timeout));
        }

        if let Some(proxy) = &config.proxy {
            agent_builder = agent_builder
                .proxy(Proxy::new(proxy).map_err(|e| Error::Esplora(Box::new(e.into())))?);
        }

        let mut blockchain = EsploraBlockchain::new(config.base_url.as_str(), config.stop_gap)
            .with_agent(agent_builder.build());

        if let Some(concurrency) = config.concurrency {
            blockchain = blockchain.with_concurrency(concurrency);
        }

        Ok(blockchain)
    }
}

impl From<ureq::Error> for EsploraError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::Status(code, _) => EsploraError::HttpResponse(code),
            e => EsploraError::Ureq(e),
        }
    }
}
