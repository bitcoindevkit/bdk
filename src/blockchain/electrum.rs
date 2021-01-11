// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

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

use self::utils::{ELSGetHistoryRes, ElectrumLikeSync};
use super::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

/// Wrapper over an Electrum Client that implements the required blockchain traits
///
/// ## Example
/// See the [`blockchain::electrum`](crate::blockchain::electrum) module for a usage example.
pub struct ElectrumBlockchain(Client);

#[cfg(test)]
#[cfg(feature = "test-electrum")]
#[bdk_blockchain_tests(crate)]
fn local_electrs() -> ElectrumBlockchain {
    ElectrumBlockchain::from(Client::new(&testutils::get_electrum_url()).unwrap())
}

impl std::convert::From<Client> for ElectrumBlockchain {
    fn from(client: Client) -> Self {
        ElectrumBlockchain(client)
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
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        self.0
            .electrum_like_setup(stop_gap, database, progress_update)
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.0.transaction_get(txid).map(Option::Some)?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self.0.transaction_broadcast(tx).map(|_| ())?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?

        Ok(self
            .0
            .block_headers_subscribe()
            .map(|data| data.height as u32)?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        Ok(FeeRate::from_btc_per_kvb(
            self.0.estimate_fee(target)? as f32
        ))
    }
}

impl ElectrumLikeSync for Client {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script> + Clone>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSGetHistoryRes>>, Error> {
        self.batch_script_get_history(scripts)
            .map(|v| {
                v.into_iter()
                    .map(|v| {
                        v.into_iter()
                            .map(
                                |electrum_client::GetHistoryRes {
                                     height, tx_hash, ..
                                 }| ELSGetHistoryRes {
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
#[derive(Debug, serde::Deserialize, serde::Serialize)]
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

        Ok(ElectrumBlockchain(Client::from_config(
            config.url.as_str(),
            electrum_config,
        )?))
    }
}
