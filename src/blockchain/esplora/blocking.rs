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
use std::ops::DerefMut;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{Transaction, Txid};

use esplora_client::{convert_fee_rate, BlockingClient, Builder, Tx};

use crate::blockchain::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

/// Structure that implements the logic to sync with Esplora
///
/// ## Example
/// See the [`blockchain::esplora`](crate::blockchain::esplora) module for a usage example.
#[derive(Debug)]
pub struct EsploraBlockchain {
    url_client: BlockingClient,
    stop_gap: usize,
    concurrency: u8,
}

impl EsploraBlockchain {
    /// Create a new instance of the client from a base URL and the `stop_gap`.
    pub fn new(base_url: &str, stop_gap: usize) -> Self {
        let url_client = Builder::new(base_url)
            .build_blocking()
            .expect("Should never fail with no proxy and timeout");

        Self::from_client(url_client, stop_gap)
    }

    /// Build a new instance given a client
    pub fn from_client(url_client: BlockingClient, stop_gap: usize) -> Self {
        EsploraBlockchain {
            url_client,
            concurrency: super::DEFAULT_CONCURRENT_REQUESTS,
            stop_gap,
        }
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
        self.url_client.broadcast(tx)?;
        Ok(())
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        let estimates = self.url_client.get_fee_estimates()?;
        Ok(FeeRate::from_sat_per_vb(convert_fee_rate(
            target, estimates,
        )?))
    }
}

impl Deref for EsploraBlockchain {
    type Target = BlockingClient;

    fn deref(&self) -> &Self::Target {
        &self.url_client
    }
}

impl StatelessBlockchain for EsploraBlockchain {}

impl GetHeight for EsploraBlockchain {
    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.url_client.get_height()?)
    }
}

impl GetTx for EsploraBlockchain {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.url_client.get_tx(txid)?)
    }
}

impl GetBlockHash for EsploraBlockchain {
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error> {
        Ok(self.url_client.get_block_hash(height as u32)?)
    }
}

impl WalletSync for EsploraBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        use crate::blockchain::script_sync::Request;
        let mut database = database.borrow_mut();
        let database = database.deref_mut();
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
                            let mut related_txs: Vec<Tx> = client.scripthash_txs(&script, None)?;

                            let n_confirmed =
                                related_txs.iter().filter(|tx| tx.status.confirmed).count();
                            // esplora pages on 25 confirmed transactions. If there's 25 or more we
                            // keep requesting to see if there's more.
                            if n_confirmed >= 25 {
                                loop {
                                    let new_related_txs: Vec<Tx> = client.scripthash_txs(
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
                                .map(Into::into)
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

impl ConfigurableBlockchain for EsploraBlockchain {
    type Config = super::EsploraBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let mut builder = Builder::new(config.base_url.as_str());

        if let Some(timeout) = config.timeout {
            builder = builder.timeout(timeout);
        }

        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(proxy);
        }

        let mut blockchain =
            EsploraBlockchain::from_client(builder.build_blocking()?, config.stop_gap);

        if let Some(concurrency) = config.concurrency {
            blockchain = blockchain.with_concurrency(concurrency);
        }

        Ok(blockchain)
    }
}
