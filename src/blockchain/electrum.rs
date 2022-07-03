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

use std::collections::{HashMap, HashSet};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{Transaction, Txid};

use electrum_client::{Client, ConfigBuilder, ElectrumApi, Socks5Config};

use super::script_sync::Request;
use super::*;
use crate::database::{BatchDatabase, Database};
use crate::error::Error;
use crate::{BlockTime, FeeRate};

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

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self.client.transaction_broadcast(tx).map(|_| ())?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        Ok(FeeRate::from_btc_per_kvb(
            self.client.estimate_fee(target)? as f32
        ))
    }
}

impl StatelessBlockchain for ElectrumBlockchain {}

impl GetHeight for ElectrumBlockchain {
    fn get_height(&self) -> Result<u32, Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?

        Ok(self
            .client
            .block_headers_subscribe()
            .map(|data| data.height as u32)?)
    }
}

impl GetTx for ElectrumBlockchain {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self.client.transaction_get(txid).map(Option::Some)?)
    }
}

impl WalletSync for ElectrumBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        let mut request = script_sync::start(database, self.stop_gap)?;
        let mut block_times = HashMap::<u32, u32>::new();
        let mut txid_to_height = HashMap::<Txid, u32>::new();
        let mut tx_cache = TxCache::new(database, &self.client);

        // Set chunk_size to the smallest value capable of finding a gap greater than stop_gap.
        let chunk_size = self.stop_gap + 1;

        // The electrum server has been inconsistent somehow in its responses during sync. For
        // example, we do a batch request of transactions and the response contains less
        // tranascations than in the request. This should never happen but we don't want to panic.
        let electrum_goof = || Error::Generic("electrum server misbehaving".to_string());

        let batch_update = loop {
            request = match request {
                Request::Script(script_req) => {
                    let scripts = script_req.request().take(chunk_size);
                    let txids_per_script: Vec<Vec<_>> = self
                        .client
                        .batch_script_get_history(scripts)
                        .map_err(Error::Electrum)?
                        .into_iter()
                        .map(|txs| {
                            txs.into_iter()
                                .map(|tx| {
                                    let tx_height = match tx.height {
                                        none if none <= 0 => None,
                                        height => {
                                            txid_to_height.insert(tx.tx_hash, height as u32);
                                            Some(height as u32)
                                        }
                                    };
                                    (tx.tx_hash, tx_height)
                                })
                                .collect()
                        })
                        .collect();

                    script_req.satisfy(txids_per_script)?
                }

                Request::Conftime(conftime_req) => {
                    // collect up to chunk_size heights to fetch from electrum
                    let needs_block_height = {
                        let mut needs_block_height = HashSet::with_capacity(chunk_size);
                        conftime_req
                            .request()
                            .filter_map(|txid| txid_to_height.get(txid).cloned())
                            .filter(|height| block_times.get(height).is_none())
                            .take(chunk_size)
                            .for_each(|height| {
                                needs_block_height.insert(height);
                            });

                        needs_block_height
                    };

                    let new_block_headers = self
                        .client
                        .batch_block_header(needs_block_height.iter().cloned())?;

                    for (height, header) in needs_block_height.into_iter().zip(new_block_headers) {
                        block_times.insert(height, header.time);
                    }

                    let conftimes = conftime_req
                        .request()
                        .take(chunk_size)
                        .map(|txid| {
                            let confirmation_time = txid_to_height
                                .get(txid)
                                .map(|height| {
                                    let timestamp =
                                        *block_times.get(height).ok_or_else(electrum_goof)?;
                                    Result::<_, Error>::Ok(BlockTime {
                                        height: *height,
                                        timestamp: timestamp.into(),
                                    })
                                })
                                .transpose()?;
                            Ok(confirmation_time)
                        })
                        .collect::<Result<_, Error>>()?;

                    conftime_req.satisfy(conftimes)?
                }
                Request::Tx(tx_req) => {
                    let needs_full = tx_req.request().take(chunk_size);
                    tx_cache.save_txs(needs_full.clone())?;
                    let full_transactions = needs_full
                        .map(|txid| tx_cache.get(*txid).ok_or_else(electrum_goof))
                        .collect::<Result<Vec<_>, _>>()?;
                    let input_txs = full_transactions.iter().flat_map(|tx| {
                        tx.input
                            .iter()
                            .filter(|input| !input.previous_output.is_null())
                            .map(|input| &input.previous_output.txid)
                    });
                    tx_cache.save_txs(input_txs)?;

                    let full_details = full_transactions
                        .into_iter()
                        .map(|tx| {
                            let mut input_index = 0usize;
                            let prev_outputs = tx
                                .input
                                .iter()
                                .map(|input| {
                                    if input.previous_output.is_null() {
                                        return Ok(None);
                                    }
                                    let prev_tx = tx_cache
                                        .get(input.previous_output.txid)
                                        .ok_or_else(electrum_goof)?;
                                    let txout = prev_tx
                                        .output
                                        .get(input.previous_output.vout as usize)
                                        .ok_or_else(electrum_goof)?;
                                    input_index += 1;
                                    Ok(Some(txout.clone()))
                                })
                                .collect::<Result<Vec<_>, Error>>()?;
                            Ok((prev_outputs, tx))
                        })
                        .collect::<Result<Vec<_>, Error>>()?;

                    tx_req.satisfy(full_details)?
                }
                Request::Finish(batch_update) => break batch_update,
            }
        };

        database.commit_batch(batch_update)?;
        Ok(())
    }
}

struct TxCache<'a, 'b, D> {
    db: &'a D,
    client: &'b Client,
    cache: HashMap<Txid, Transaction>,
}

impl<'a, 'b, D: Database> TxCache<'a, 'b, D> {
    fn new(db: &'a D, client: &'b Client) -> Self {
        TxCache {
            db,
            client,
            cache: HashMap::default(),
        }
    }
    fn save_txs<'c>(&mut self, txids: impl Iterator<Item = &'c Txid>) -> Result<(), Error> {
        let mut need_fetch = vec![];
        for txid in txids {
            if self.cache.get(txid).is_some() {
                continue;
            } else if let Some(transaction) = self.db.get_raw_tx(txid)? {
                self.cache.insert(*txid, transaction);
            } else {
                need_fetch.push(txid);
            }
        }

        if !need_fetch.is_empty() {
            let txs = self
                .client
                .batch_transaction_get(need_fetch.clone())
                .map_err(Error::Electrum)?;
            for (tx, _txid) in txs.into_iter().zip(need_fetch) {
                debug_assert_eq!(*_txid, tx.txid());
                self.cache.insert(tx.txid(), tx);
            }
        }

        Ok(())
    }

    fn get(&self, txid: Txid) -> Option<Transaction> {
        self.cache.get(&txid).map(Clone::clone)
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
mod test {
    use std::sync::Arc;

    use super::*;
    use crate::database::MemoryDatabase;
    use crate::testutils::blockchain_tests::TestClient;
    use crate::wallet::{AddressIndex, Wallet};

    crate::bdk_blockchain_tests! {
        fn test_instance(test_client: &TestClient) -> ElectrumBlockchain {
            ElectrumBlockchain::from(Client::new(&test_client.electrsd.electrum_url).unwrap())
        }
    }

    fn get_factory() -> (TestClient, Arc<ElectrumBlockchain>) {
        let test_client = TestClient::default();

        let factory = Arc::new(ElectrumBlockchain::from(
            Client::new(&test_client.electrsd.electrum_url).unwrap(),
        ));

        (test_client, factory)
    }

    #[test]
    fn test_electrum_blockchain_factory() {
        let (_test_client, factory) = get_factory();

        let a = factory.build("aaaaaa", None).unwrap();
        let b = factory.build("bbbbbb", None).unwrap();

        assert_eq!(
            a.client.block_headers_subscribe().unwrap().height,
            b.client.block_headers_subscribe().unwrap().height
        );
    }

    #[test]
    fn test_electrum_blockchain_factory_sync_wallet() {
        let (mut test_client, factory) = get_factory();

        let db = MemoryDatabase::new();
        let wallet = Wallet::new(
            "wpkh(L5EZftvrYaSudiozVRzTqLcHLNDoVn7H5HSfM9BAN6tMJX8oTWz6)",
            None,
            bitcoin::Network::Regtest,
            db,
        )
        .unwrap();

        let address = wallet.get_address(AddressIndex::New).unwrap();

        let tx = testutils! {
            @tx ( (@addr address.address) => 50_000 )
        };
        test_client.receive(tx);

        factory
            .sync_wallet(&wallet, None, Default::default())
            .unwrap();

        assert_eq!(wallet.get_balance().unwrap(), 50_000);
    }

    #[test]
    fn test_electrum_blockchain_factory_sync_with_stop_gaps() {
        // Test whether Electrum blockchain syncs with expected behaviour given different `stop_gap`
        // parameters.
        //
        // For each test vector:
        // * Fill wallet's derived addresses with balances (as specified by test vector).
        //    * [0..addrs_before]          => 1000sats for each address
        //    * [addrs_before..actual_gap] => empty addresses
        //    * [actual_gap..addrs_after]  => 1000sats for each address
        // * Then, perform wallet sync and obtain wallet balance
        // * Check balance is within expected range (we can compare `stop_gap` and `actual_gap` to
        //    determine this).

        // Generates wallet descriptor
        let descriptor_of_account = |account_index: usize| -> String {
            format!("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/{account_index}/*)")
        };

        // Amount (in satoshis) provided to a single address (which expects to have a balance)
        const AMOUNT_PER_TX: u64 = 1000;

        // [stop_gap, actual_gap, addrs_before, addrs_after]
        //
        // [0]     stop_gap: Passed to [`ElectrumBlockchainConfig`]
        // [1]   actual_gap: Range size of address indexes without a balance
        // [2] addrs_before: Range size of address indexes (before gap) which contains a balance
        // [3]  addrs_after: Range size of address indexes (after gap) which contains a balance
        let test_vectors: Vec<[u64; 4]> = vec![
            [0, 0, 0, 5],
            [0, 0, 5, 5],
            [0, 1, 5, 5],
            [0, 2, 5, 5],
            [1, 0, 5, 5],
            [1, 1, 5, 5],
            [1, 2, 5, 5],
            [2, 1, 5, 5],
            [2, 2, 5, 5],
            [2, 3, 5, 5],
        ];

        let mut test_client = TestClient::default();

        for (account_index, vector) in test_vectors.into_iter().enumerate() {
            let [stop_gap, actual_gap, addrs_before, addrs_after] = vector;
            let descriptor = descriptor_of_account(account_index);

            let factory = Arc::new(
                ElectrumBlockchain::from_config(&ElectrumBlockchainConfig {
                    url: test_client.electrsd.electrum_url.clone(),
                    socks5: None,
                    retry: 0,
                    timeout: None,
                    stop_gap: stop_gap as _,
                })
                .unwrap(),
            );
            let wallet = Wallet::new(
                descriptor.as_str(),
                None,
                bitcoin::Network::Regtest,
                MemoryDatabase::new(),
            )
            .unwrap();

            // fill server-side with txs to specified address indexes
            // return the max balance of the wallet (also the actual balance)
            let max_balance = (0..addrs_before)
                .chain(addrs_before + actual_gap..addrs_before + actual_gap + addrs_after)
                .fold(0_u64, |sum, i| {
                    let address = wallet.get_address(AddressIndex::Peek(i as _)).unwrap();
                    let tx = testutils! {
                        @tx ( (@addr address.address) => AMOUNT_PER_TX )
                    };
                    test_client.receive(tx);
                    sum + AMOUNT_PER_TX
                });

            // generate blocks to confirm new transactions
            test_client.generate(3, None);

            // minimum allowed balance of wallet (based on stop gap)
            let min_balance = if actual_gap > stop_gap {
                addrs_before * AMOUNT_PER_TX
            } else {
                max_balance
            };

            // perform wallet sync
            factory
                .sync_wallet(&wallet, None, Default::default())
                .unwrap();

            let wallet_balance = wallet.get_balance().unwrap();

            let details = format!(
                "test_vector: [stop_gap: {}, actual_gap: {}, addrs_before: {}, addrs_after: {}]",
                stop_gap, actual_gap, addrs_before, addrs_after,
            );
            assert!(
                wallet_balance <= max_balance,
                "wallet balance is greater than received amount: {}",
                details
            );
            assert!(
                wallet_balance >= min_balance,
                "wallet balance is smaller than expected: {}",
                details
            );
        }
    }
}
