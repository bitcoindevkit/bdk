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
//! This module defines an [`OnlineBlockchain`] struct that wraps an [`electrum_client::Client`]
//! and implements the logic required to populate the wallet's [database](crate::database::Database) by
//! querying the inner client.
//!
//! ## Example
//!
//! ```no_run
//! # use magical_bitcoin_wallet::blockchain::electrum::ElectrumBlockchain;
//! let client = electrum_client::Client::new("ssl://electrum.blockstream.info:50002", None)?;
//! let blockchain = ElectrumBlockchain::from(client);
//! # Ok::<(), magical_bitcoin_wallet::error::Error>(())
//! ```

use std::collections::HashSet;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{Script, Transaction, Txid};

use electrum_client::{Client, ElectrumApi};

use self::utils::{ELSGetHistoryRes, ELSListUnspentRes, ElectrumLikeSync};
use super::*;
use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

/// Wrapper over an Electrum Client that implements the required blockchain traits
///
/// ## Example
/// See the [`blockchain::electrum`](crate::blockchain::electrum) module for a usage example.
pub struct ElectrumBlockchain(Option<Client>);

#[cfg(test)]
#[cfg(feature = "test-electrum")]
#[magical_blockchain_tests(crate)]
fn local_electrs() -> ElectrumBlockchain {
    ElectrumBlockchain::from(Client::new(&testutils::get_electrum_url(), None).unwrap())
}

impl std::convert::From<Client> for ElectrumBlockchain {
    fn from(client: Client) -> Self {
        ElectrumBlockchain(Some(client))
    }
}

impl Blockchain for ElectrumBlockchain {
    fn offline() -> Self {
        ElectrumBlockchain(None)
    }

    fn is_online(&self) -> bool {
        self.0.is_some()
    }
}

impl OnlineBlockchain for ElectrumBlockchain {
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
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .electrum_like_setup(stop_gap, database, progress_update)
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .transaction_get(txid)
            .map(Option::Some)?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .transaction_broadcast(tx)
            .map(|_| ())?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?

        Ok(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .block_headers_subscribe()
            .map(|data| data.height as u32)?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        Ok(FeeRate::from_btc_per_kvb(
            self.0
                .as_ref()
                .ok_or(Error::OfflineClient)?
                .estimate_fee(target)? as f32,
        ))
    }
}

impl ElectrumLikeSync for Client {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script>>(
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

    fn els_batch_script_list_unspent<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSListUnspentRes>>, Error> {
        self.batch_script_list_unspent(scripts)
            .map(|v| {
                v.into_iter()
                    .map(|v| {
                        v.into_iter()
                            .map(
                                |electrum_client::ListUnspentRes {
                                     height,
                                     tx_hash,
                                     tx_pos,
                                     ..
                                 }| ELSListUnspentRes {
                                    height,
                                    tx_hash,
                                    tx_pos,
                                },
                            )
                            .collect()
                    })
                    .collect()
            })
            .map_err(Error::Electrum)
    }

    fn els_transaction_get(&self, txid: &Txid) -> Result<Transaction, Error> {
        self.transaction_get(txid).map_err(Error::Electrum)
    }
}
