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

//! Electrum
//!
//! This module defines a [`Blockchain`] struct that wraps an [`electrum_client::Client`]
//! and implements the logic required to populate the wallet's [database](crate::database::Database) by
//! querying the inner client.
//!
//! ## Example
//!
//! ```no_run
//! # use bdk::blockchain::electrum::ElectrumBlockchain;
//! let client = electrum_client::Client::new("ssl://electrum.blockstream.info:50002")?;
//! let blockchain = ElectrumBlockchain::from(client);
//! # Ok::<(), bdk::Error>(())
//! ```

use std::collections::HashSet;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{BlockHeader, Script, Transaction, Txid};

use electrum_client::{Client, ConfigBuilder, ElectrumApi, Socks5Config};

use self::utils::{ElectrumLikeSync, ElsGetHistoryRes};
use super::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

/// Wrapper over an Electrum Client that implements the required blockchain traits
///
/// ## Example
/// See the [`blockchain::electrum`](crate::blockchain::electrum) module for a usage example.
pub struct ElectrumBlockchain {
    client: Client,
    stop_gap: usize,
}

impl std::convert::From<Client> for ElectrumBlockchain {
    fn from(client: Client) -> Self {
        ElectrumBlockchain {
            client,
            stop_gap: 20,
        }
    }
}

impl Blockchain for ElectrumBlockchain {
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
        self.client
            .electrum_like_setup(self.stop_gap, database, progress_update)
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.client.transaction_get(txid).map(Option::Some)?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self.client.transaction_broadcast(tx).map(|_| ())?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?

        Ok(self
            .client
            .block_headers_subscribe()
            .map(|data| data.height as u32)?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        Ok(FeeRate::from_btc_per_kvb(
            self.client.estimate_fee(target)? as f32
        ))
    }
}

impl ElectrumLikeSync for Client {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script> + Clone>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ElsGetHistoryRes>>, Error> {
        self.batch_script_get_history(scripts)
            .map(|v| {
                v.into_iter()
                    .map(|v| {
                        v.into_iter()
                            .map(
                                |electrum_client::GetHistoryRes {
                                     height, tx_hash, ..
                                 }| ElsGetHistoryRes {
                                    height,
                                    tx_hash,
                                },
                            )
                            .collect()
                    })
                    .collect()
            })
            .map_err(Error::Electrum)
    }

    fn els_batch_transaction_get<'s, I: IntoIterator<Item = &'s Txid> + Clone>(
        &self,
        txids: I,
    ) -> Result<Vec<Transaction>, Error> {
        self.batch_transaction_get(txids).map_err(Error::Electrum)
    }

    fn els_batch_block_header<I: IntoIterator<Item = u32> + Clone>(
        &self,
        heights: I,
    ) -> Result<Vec<BlockHeader>, Error> {
        self.batch_block_header(heights).map_err(Error::Electrum)
    }
}

/// Configuration for an [`ElectrumBlockchain`]
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct ElectrumBlockchainConfig {
    /// URL of the Electrum server (such as ElectrumX, Esplora, BWT) may start with `ssl://` or `tcp://` and include a port
    ///
    /// eg. `ssl://electrum.blockstream.info:60002`
    pub url: String,
    /// URL of the socks5 proxy server or a Tor service
    pub socks5: Option<String>,
    /// Request retry count
    pub retry: u8,
    /// Request timeout (seconds)
    pub timeout: Option<u8>,
    /// Stop searching addresses for transactions after finding an unused gap of this length
    pub stop_gap: usize,
}

impl ConfigurableBlockchain for ElectrumBlockchain {
    type Config = ElectrumBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let socks5 = config.socks5.as_ref().map(Socks5Config::new);
        let electrum_config = ConfigBuilder::new()
            .retry(config.retry)
            .timeout(config.timeout)?
            .socks5(socks5)?
            .build();

        Ok(ElectrumBlockchain {
            client: Client::from_config(config.url.as_str(), electrum_config)?,
            stop_gap: config.stop_gap,
        })
    }
}

#[cfg(test)]
#[cfg(feature = "test-electrum")]
crate::bdk_blockchain_tests! {
    fn test_instance(test_client: &TestClient) -> ElectrumBlockchain {
        ElectrumBlockchain::from(Client::new(&test_client.electrsd.electrum_url).unwrap())
    }
}
