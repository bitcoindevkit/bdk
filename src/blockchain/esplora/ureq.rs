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

use ureq::{Agent, Response};

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::hashes::{sha256, Hash};
use bitcoin::{BlockHeader, Script, Transaction, Txid};

use crate::blockchain::esplora::{EsploraError, EsploraGetHistory};
use crate::blockchain::utils::{ElectrumLikeSync, ElsGetHistoryRes};
use crate::blockchain::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

#[derive(Debug)]
struct UrlClient {
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
                agent: Agent::new(),
            },
            stop_gap,
        }
    }

    /// Set the inner `ureq` agent.
    pub fn with_agent(mut self, agent: Agent) -> Self {
        self.url_client.agent = agent;
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

    fn setup<D: BatchDatabase, P: Progress>(
        &self,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        self.url_client
            .electrum_like_setup(self.stop_gap, database, progress_update)
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.url_client._get_tx(txid)?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        let _txid = self.url_client._broadcast(tx)?;
        Ok(())
    }

    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.url_client._get_height()?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        let estimates = self.url_client._get_fee_estimates()?;

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

    fn _script_get_history(&self, script: &Script) -> Result<Vec<ElsGetHistoryRes>, EsploraError> {
        let mut result = Vec::new();
        let scripthash = Self::script_to_scripthash(script);

        // Add the unconfirmed transactions first

        let resp = self
            .agent
            .get(&format!(
                "{}/scripthash/{}/txs/mempool",
                self.url, scripthash
            ))
            .call();

        let v = match resp {
            Ok(resp) => {
                let v: Vec<EsploraGetHistory> = resp.into_json()?;
                Ok(v)
            }
            Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
            Err(e) => Err(EsploraError::Ureq(e)),
        }?;

        result.extend(v.into_iter().map(|x| ElsGetHistoryRes {
            tx_hash: x.txid,
            height: x.status.block_height.unwrap_or(0) as i32,
        }));

        debug!(
            "Found {} mempool txs for {} - {:?}",
            result.len(),
            scripthash,
            script
        );

        // Then go through all the pages of confirmed transactions
        let mut last_txid = String::new();
        loop {
            let resp = self
                .agent
                .get(&format!(
                    "{}/scripthash/{}/txs/chain/{}",
                    self.url, scripthash, last_txid
                ))
                .call();

            let v = match resp {
                Ok(resp) => {
                    let v: Vec<EsploraGetHistory> = resp.into_json()?;
                    Ok(v)
                }
                Err(ureq::Error::Status(code, _)) => Err(EsploraError::HttpResponse(code)),
                Err(e) => Err(EsploraError::Ureq(e)),
            }?;

            let len = v.len();
            if let Some(elem) = v.last() {
                last_txid = elem.txid.to_hex();
            }

            debug!("... adding {} confirmed transactions", len);

            result.extend(v.into_iter().map(|x| ElsGetHistoryRes {
                tx_hash: x.txid,
                height: x.status.block_height.unwrap_or(0) as i32,
            }));

            if len < 25 {
                break;
            }
        }

        Ok(result)
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

impl ElectrumLikeSync for UrlClient {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ElsGetHistoryRes>>, Error> {
        let mut results = vec![];
        for script in scripts.into_iter() {
            let v = self._script_get_history(script)?;
            results.push(v);
        }
        Ok(results)
    }

    fn els_batch_transaction_get<'s, I: IntoIterator<Item = &'s Txid>>(
        &self,
        txids: I,
    ) -> Result<Vec<Transaction>, Error> {
        let mut results = vec![];
        for txid in txids.into_iter() {
            let tx = self._get_tx_no_opt(txid)?;
            results.push(tx);
        }
        Ok(results)
    }

    fn els_batch_block_header<I: IntoIterator<Item = u32>>(
        &self,
        heights: I,
    ) -> Result<Vec<BlockHeader>, Error> {
        let mut results = vec![];
        for height in heights.into_iter() {
            let header = self._get_header(height)?;
            results.push(header);
        }
        Ok(results)
    }
}

/// Configuration for an [`EsploraBlockchain`]
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct EsploraBlockchainConfig {
    /// Base URL of the esplora service eg. `https://blockstream.info/api/`
    pub base_url: String,
    /// Socket read timeout.
    pub timeout_read: u64,
    /// Socket write timeout.
    pub timeout_write: u64,
    /// Stop searching addresses for transactions after finding an unused gap of this length.
    pub stop_gap: usize,
}

impl ConfigurableBlockchain for EsploraBlockchain {
    type Config = EsploraBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let agent: Agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(config.timeout_read))
            .timeout_write(Duration::from_secs(config.timeout_write))
            .build();
        Ok(EsploraBlockchain::new(config.base_url.as_str(), config.stop_gap).with_agent(agent))
    }
}
