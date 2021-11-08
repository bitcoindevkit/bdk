// Bitcoin Dev Kit
// Written in 2021 by Riccardo Casatta <riccardo@casatta.it>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Rpc Blockchain
//!
//! Backend that gets blockchain data from Bitcoin Core RPC
//!
//! This is an **EXPERIMENTAL** feature, API and other major changes are expected.
//!
//! ## Example
//!
//! ```no_run
//! # use bdk::blockchain::{RpcConfig, RpcBlockchain, ConfigurableBlockchain, rpc::Auth};
//! let config = RpcConfig {
//!     url: "127.0.0.1:18332".to_string(),
//!     auth: Auth::Cookie {
//!         file: "/home/user/.bitcoin/.cookie".into(),
//!     },
//!     network: bdk::bitcoin::Network::Testnet,
//!     wallet_name: "wallet_name".to_string(),
//!     skip_blocks: None,
//! };
//! let blockchain = RpcBlockchain::from_config(&config);
//! ```

use crate::bitcoin::consensus::deserialize;
use crate::bitcoin::{Address, Network, Transaction, Txid};
use crate::blockchain::{
    script_sync::Request, Blockchain, Capability, ConfigurableBlockchain, Progress,
};
use crate::database::{BatchDatabase, DatabaseUtils};
use crate::descriptor::{get_checksum, IntoWalletDescriptor};
use crate::wallet::utils::SecpCtx;
use crate::{ConfirmationTime, Error, FeeRate, KeychainKind};
use bitcoincore_rpc::json::{
    GetAddressInfoResultLabel, ImportMultiOptions, ImportMultiRequest,
    ImportMultiRequestScriptPubkey, ImportMultiRescanSince,
};
use bitcoincore_rpc::jsonrpc::serde_json::Value;
use bitcoincore_rpc::Auth as RpcAuth;
use bitcoincore_rpc::{Client, RpcApi};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

// Default stop gap at which the sync loop terminates
const DEFAULT_STOP_GAP: u32 = 20;

/// The main struct for RPC backend implementing the [crate::blockchain::Blockchain] trait
#[derive(Debug)]
pub struct RpcBlockchain {
    /// Rpc client to the node, includes the wallet name
    client: Client,
    /// Network used
    network: Network,
    /// Blockchain capabilities, cached here at startup
    capabilities: HashSet<Capability>,
    /// Skip this many blocks of the blockchain at the first rescan, if None the rescan is done from the genesis block
    skip_blocks: Option<u32>,
    /// Optional stop gap in address chain to stop syncing. Default = 20.
    stop_gap: Option<u32>,

    /// This is a fixed Address used as a hack key to store information on the node
    _storage_address: Address,
}

/// RpcBlockchain configuration options
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RpcConfig {
    /// The bitcoin node url
    pub url: String,
    /// The bitcoin node authentication mechanism
    pub auth: Auth,
    /// The network we are using (it will be checked the bitcoin node network matches this)
    pub network: Network,
    /// The wallet name in the bitcoin node, consider using [wallet_name_from_descriptor] for this
    pub wallet_name: String,
    /// Skip this many blocks of the blockchain at the first rescan, if None the rescan is done from the genesis block
    pub skip_blocks: Option<u32>,
    /// Optional stop gap in address chain to stop syncing. Default = 20.
    pub stop_gap: Option<u32>,
}

/// This struct is equivalent to [bitcoincore_rpc::Auth] but it implements [serde::Serialize]
/// To be removed once upstream equivalent is implementing Serialize (json serialization format
/// should be the same), see [rust-bitcoincore-rpc/pull/181](https://github.com/rust-bitcoin/rust-bitcoincore-rpc/pull/181)
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub enum Auth {
    /// None authentication
    None,
    /// Authentication with username and password, usually [Auth::Cookie] should be preferred
    UserPass {
        /// Username
        username: String,
        /// Password
        password: String,
    },
    /// Authentication with a cookie file
    Cookie {
        /// Cookie file
        file: PathBuf,
    },
}

impl From<Auth> for RpcAuth {
    fn from(auth: Auth) -> Self {
        match auth {
            Auth::None => RpcAuth::None,
            Auth::UserPass { username, password } => RpcAuth::UserPass(username, password),
            Auth::Cookie { file } => RpcAuth::CookieFile(file),
        }
    }
}

impl RpcBlockchain {
    fn get_node_synced_height(&self) -> Result<u32, Error> {
        let info = self.client.get_address_info(&self._storage_address)?;
        if let Some(GetAddressInfoResultLabel::Simple(label)) = info.labels.first() {
            Ok(label
                .parse::<u32>()
                .unwrap_or_else(|_| self.skip_blocks.unwrap_or(0)))
        } else {
            Ok(self.skip_blocks.unwrap_or(0))
        }
    }

    /// Set the synced height in the core node by using a label of a fixed address so that
    /// another client with the same descriptor doesn't rescan the blockchain
    fn set_node_synced_height(&self, height: u32) -> Result<(), Error> {
        Ok(self
            .client
            .set_label(&self._storage_address, &height.to_string())?)
    }
}

impl Blockchain for RpcBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        self.capabilities.clone()
    }

    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        let mut scripts_pubkeys = database.iter_script_pubkeys(Some(KeychainKind::External))?;
        scripts_pubkeys.extend(database.iter_script_pubkeys(Some(KeychainKind::Internal))?);
        debug!(
            "importing {} script_pubkeys (some maybe already imported)",
            scripts_pubkeys.len()
        );
        let requests: Vec<_> = scripts_pubkeys
            .iter()
            .map(|s| ImportMultiRequest {
                timestamp: ImportMultiRescanSince::Timestamp(0),
                script_pubkey: Some(ImportMultiRequestScriptPubkey::Script(s)),
                watchonly: Some(true),
                ..Default::default()
            })
            .collect();
        let options = ImportMultiOptions {
            rescan: Some(false),
        };
        // Note we use import_multi because as of bitcoin core 0.21.0 many descriptors are not supported
        // https://bitcoindevkit.org/descriptors/#compatibility-matrix
        //TODO maybe convenient using import_descriptor for compatible descriptor and import_multi as fallback
        self.client.import_multi(&requests, Some(&options))?;

        loop {
            let current_height = self.get_height()?;

            // min because block invalidate may cause height to go down
            let node_synced = self.get_node_synced_height()?.min(current_height);

            let sync_up_to = node_synced.saturating_add(10_000).min(current_height);

            debug!("rescan_blockchain from:{} to:{}", node_synced, sync_up_to);
            self.client
                .rescan_blockchain(Some(node_synced as usize), Some(sync_up_to as usize))?;
            progress_update.update((sync_up_to as f32) / (current_height as f32), None)?;

            self.set_node_synced_height(sync_up_to)?;

            if sync_up_to == current_height {
                break;
            }
        }

        self.sync(database, progress_update)
    }

    fn sync<D: BatchDatabase, P: 'static + Progress>(
        &self,
        db: &mut D,
        _progress_update: P,
    ) -> Result<(), Error> {
        // internal cache to speed up conftime lookups
        let mut txid_conftime_map = BTreeMap::new();

        let mut request =
            super::script_sync::start(db, self.stop_gap.unwrap_or(DEFAULT_STOP_GAP) as usize)?;

        // Fetch txs from core in batches of 1000 txs
        // Stop when there's no more
        let mut listed_txs = vec![];
        let mut fetch_count = 0;
        loop {
            let fetched_txs = self.client.list_transactions(
                None,
                Some((fetch_count + 1) * 1000),
                Some(fetch_count * 1000),
                Some(true),
            )?;
            listed_txs.extend(fetched_txs.clone());
            if fetched_txs.len() < 1000 {
                break;
            }
            fetch_count += 1;
        }

        // list_transactions returns conflicting txs too.
        // Filter out conflicting: confirmation < 0 and not in mempool.
        let listed_txs = listed_txs
            .iter()
            .filter(|t| {
                t.info.confirmations > 0 || self.client.get_mempool_entry(&t.info.txid).is_ok()
            })
            .collect::<Vec<_>>();

        let batch_update = loop {
            request = match request {
                Request::Script(script_req) => {
                    let scripts = script_req.request();

                    let satisfier = scripts
                        .map(|sk| {
                            let related_txs = listed_txs
                                .iter()
                                .filter(|tx_result| {
                                    // Filter out txs related to the script_pubkey
                                    if let Some(address) = &tx_result.detail.address {
                                        if address.script_pubkey() == *sk {
                                            // If conftime data is available, cache them.
                                            if let (Some(height), Some(timestamp)) = (
                                                tx_result.info.blockheight,
                                                tx_result.info.blocktime,
                                            ) {
                                                txid_conftime_map.insert(
                                                    tx_result.info.txid,
                                                    ConfirmationTime { height, timestamp },
                                                );
                                            }
                                            true
                                        } else {
                                            // address doesn't match
                                            false
                                        }
                                    } else {
                                        // There is no address associated with this tx
                                        false
                                    }
                                })
                                .map(|tx_result| (tx_result.info.txid, tx_result.info.blockheight))
                                .collect::<Vec<_>>();
                            related_txs
                        })
                        .collect::<Vec<_>>();

                    script_req.satisfy(satisfier)?
                }

                Request::Tx(tx_req) => {
                    let tx_needed = tx_req.request();

                    let satisfier = tx_needed
                        .map(|txid| {
                            let full_tx = self.client.get_transaction(txid, Some(true))?;
                            let tx = deserialize::<bitcoin::Transaction>(&full_tx.hex)?;
                            let prev_outs = tx
                                .input
                                .iter()
                                .map(|txin| {
                                    // check if the prev_out is in db
                                    if let Some(txout) =
                                        db.get_previous_output(&txin.previous_output)?
                                    {
                                        Ok(Some(txout))
                                    } else {
                                        // if not, try to fetch the prev_out
                                        if let Ok(tx) = self
                                            .client
                                            .get_raw_transaction(&txin.previous_output.txid, None)
                                        {
                                            Ok(Some(
                                                tx.output[txin.previous_output.vout as usize]
                                                    .clone(),
                                            ))
                                        } else {
                                            // There is no prev_out, probably it's a coinbase
                                            Ok(None)
                                        }
                                    }
                                })
                                .collect::<Result<Vec<_>, Error>>()?;
                            Ok((prev_outs, tx))
                        })
                        .collect::<Result<Vec<_>, Error>>()?;

                    tx_req.satisfy(satisfier)?
                }

                Request::Conftime(conftime_req) => {
                    let txids = conftime_req.request();

                    let satisfier = txids
                        .map(|txid| {
                            // first check in the cache
                            if let Some(conf_time) = txid_conftime_map.get(txid) {
                                Ok(Some(conf_time.to_owned()))
                            // if not found, ask for more details
                            } else {
                                let tx_details = self.client.get_transaction(txid, Some(true))?;
                                match (tx_details.info.blockheight, tx_details.info.blocktime) {
                                    (Some(height), Some(timestamp)) => {
                                        Ok(Some(ConfirmationTime { height, timestamp }))
                                    }
                                    // the tx is still unconfirmed
                                    _ => Ok(None),
                                }
                            }
                        })
                        .collect::<Result<Vec<_>, Error>>()?;

                    conftime_req.satisfy(satisfier)?
                }

                Request::Finish(batch_update) => break batch_update,
            }
        };

        db.commit_batch(batch_update)?;
        Ok(())
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(Some(self.client.get_raw_transaction(txid, None)?))
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self.client.send_raw_transaction(tx).map(|_| ())?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.client.get_blockchain_info().map(|i| i.blocks as u32)?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        let sat_per_kb = self
            .client
            .estimate_smart_fee(target as u16, None)?
            .fee_rate
            .ok_or(Error::FeeRateUnavailable)?
            .as_sat() as f64;

        Ok(FeeRate::from_sat_per_vb((sat_per_kb / 1000f64) as f32))
    }
}

impl ConfigurableBlockchain for RpcBlockchain {
    type Config = RpcConfig;

    /// Returns RpcBlockchain backend creating an RPC client to a specific wallet named as the descriptor's checksum
    /// if it's the first time it creates the wallet in the node and upon return is granted the wallet is loaded
    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let wallet_name = config.wallet_name.clone();
        let wallet_url = format!("{}/wallet/{}", config.url, &wallet_name);
        debug!("connecting to {} auth:{:?}", wallet_url, config.auth);

        let client = Client::new(wallet_url.as_str(), config.auth.clone().into())?;
        let loaded_wallets = client.list_wallets()?;
        if loaded_wallets.contains(&wallet_name) {
            debug!("wallet already loaded {:?}", wallet_name);
        } else {
            let existing_wallets = list_wallet_dir(&client)?;
            if existing_wallets.contains(&wallet_name) {
                client.load_wallet(&wallet_name)?;
                debug!("wallet loaded {:?}", wallet_name);
            } else {
                client.create_wallet(&wallet_name, Some(true), None, None, None)?;
                debug!("wallet created {:?}", wallet_name);
            }
        }

        let blockchain_info = client.get_blockchain_info()?;
        let network = match blockchain_info.chain.as_str() {
            "main" => Network::Bitcoin,
            "test" => Network::Testnet,
            "regtest" => Network::Regtest,
            "signet" => Network::Signet,
            _ => return Err(Error::Generic("Invalid network".to_string())),
        };
        if network != config.network {
            return Err(Error::InvalidNetwork {
                requested: config.network,
                found: network,
            });
        }

        let mut capabilities: HashSet<_> = vec![Capability::FullHistory].into_iter().collect();
        let rpc_version = client.version()?;
        if rpc_version >= 210_000 {
            let info: HashMap<String, Value> = client.call("getindexinfo", &[]).unwrap();
            if info.contains_key("txindex") {
                capabilities.insert(Capability::GetAnyTx);
                capabilities.insert(Capability::AccurateFees);
            }
        }

        // this is just a fixed address used only to store a label containing the synced height in the node
        let mut storage_address =
            Address::from_str("bc1qst0rewf0wm4kw6qn6kv0e5tc56nkf9yhcxlhqv").unwrap();
        storage_address.network = network;

        Ok(RpcBlockchain {
            client,
            network,
            capabilities,
            _storage_address: storage_address,
            skip_blocks: config.skip_blocks,
            stop_gap: Some(config.stop_gap.unwrap_or(DEFAULT_STOP_GAP)),
        })
    }
}

/// Deterministically generate a unique name given the descriptors defining the wallet
pub fn wallet_name_from_descriptor<T>(
    descriptor: T,
    change_descriptor: Option<T>,
    network: Network,
    secp: &SecpCtx,
) -> Result<String, Error>
where
    T: IntoWalletDescriptor,
{
    //TODO check descriptors contains only public keys
    let descriptor = descriptor
        .into_wallet_descriptor(secp, network)?
        .0
        .to_string();
    let mut wallet_name = get_checksum(&descriptor[..descriptor.find('#').unwrap()])?;
    if let Some(change_descriptor) = change_descriptor {
        let change_descriptor = change_descriptor
            .into_wallet_descriptor(secp, network)?
            .0
            .to_string();
        wallet_name.push_str(
            get_checksum(&change_descriptor[..change_descriptor.find('#').unwrap()])?.as_str(),
        );
    }

    Ok(wallet_name)
}

/// return the wallets available in default wallet directory
//TODO use bitcoincore_rpc method when PR #179 lands
fn list_wallet_dir(client: &Client) -> Result<Vec<String>, Error> {
    #[derive(Deserialize)]
    struct Name {
        name: String,
    }
    #[derive(Deserialize)]
    struct CallResult {
        wallets: Vec<Name>,
    }

    let result: CallResult = client.call("listwalletdir", &[])?;
    Ok(result.wallets.into_iter().map(|n| n.name).collect())
}

#[cfg(test)]
#[cfg(feature = "test-rpc")]
crate::bdk_blockchain_tests! {

    fn test_instance(test_client: &TestClient) -> RpcBlockchain {
        let config = RpcConfig {
            url: test_client.bitcoind.rpc_url(),
            auth: Auth::Cookie { file: test_client.bitcoind.params.cookie_file.clone() },
            network: Network::Regtest,
            wallet_name: format!("client-wallet-test-{:?}", std::time::SystemTime::now() ),
            skip_blocks: None,
            stop_gap: None,
        };
        RpcBlockchain::from_config(&config).unwrap()
    }
}
