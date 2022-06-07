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
use crate::bitcoin::hashes::hex::ToHex;
use crate::bitcoin::{Address, Network, OutPoint, Transaction, TxOut, Txid};
use crate::blockchain::*;
use crate::database::{BatchDatabase, DatabaseUtils};
use crate::descriptor::get_checksum;
use crate::{BlockTime, Error, FeeRate, KeychainKind, LocalUtxo, TransactionDetails};
use bitcoincore_rpc::json::{
    GetAddressInfoResultLabel, ImportMultiOptions, ImportMultiRequest,
    ImportMultiRequestScriptPubkey, ImportMultiRescanSince,
};
use bitcoincore_rpc::jsonrpc::serde_json::{json, Value};
use bitcoincore_rpc::Auth as RpcAuth;
use bitcoincore_rpc::{Client, RpcApi};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

/// The main struct for RPC backend implementing the [crate::blockchain::Blockchain] trait
#[derive(Debug)]
pub struct RpcBlockchain {
    /// Rpc client to the node, includes the wallet name
    client: Client,
    /// Whether the wallet is a "descriptor" or "legacy" wallet in Core
    is_descriptors: bool,
    /// Blockchain capabilities, cached here at startup
    capabilities: HashSet<Capability>,
    /// Skip this many blocks of the blockchain at the first rescan, if None the rescan is done from the genesis block
    skip_blocks: Option<u32>,

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
    /// The wallet name in the bitcoin node, consider using [crate::wallet::wallet_name_from_descriptor] for this
    pub wallet_name: String,
    /// Skip this many blocks of the blockchain at the first rescan, if None the rescan is done from the genesis block
    pub skip_blocks: Option<u32>,
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

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self.client.send_raw_transaction(tx).map(|_| ())?)
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

impl GetTx for RpcBlockchain {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(Some(self.client.get_raw_transaction(txid, None)?))
    }
}

impl GetHeight for RpcBlockchain {
    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.client.get_blockchain_info().map(|i| i.blocks as u32)?)
    }
}

impl WalletSync for RpcBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &mut D,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        let mut scripts_pubkeys = database.iter_script_pubkeys(Some(KeychainKind::External))?;
        scripts_pubkeys.extend(database.iter_script_pubkeys(Some(KeychainKind::Internal))?);
        debug!(
            "importing {} script_pubkeys (some maybe already imported)",
            scripts_pubkeys.len()
        );

        if self.is_descriptors {
            // Core still doesn't support complex descriptors like BDK, but when the wallet type is
            // "descriptors" we should import individual addresses using `importdescriptors` rather
            // than `importmulti`, using the `raw()` descriptor which allows us to specify an
            // arbitrary script
            let requests = Value::Array(
                scripts_pubkeys
                    .iter()
                    .map(|s| {
                        let desc = format!("raw({})", s.to_hex());
                        json!({
                            "timestamp": "now",
                            "desc": format!("{}#{}", desc, get_checksum(&desc).unwrap()),
                        })
                    })
                    .collect(),
            );

            let res: Vec<Value> = self.client.call("importdescriptors", &[requests])?;
            res.into_iter()
                .map(|v| match v["success"].as_bool() {
                    Some(true) => Ok(()),
                    Some(false) => Err(Error::Generic(
                        v["error"]["message"]
                            .as_str()
                            .unwrap_or("Unknown error")
                            .to_string(),
                    )),
                    _ => Err(Error::Generic("Unexpected response from Core".to_string())),
                })
                .collect::<Result<Vec<_>, _>>()?;
        } else {
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
            self.client.import_multi(&requests, Some(&options))?;
        }

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

        self.wallet_sync(database, progress_update)
    }

    fn wallet_sync<D: BatchDatabase>(
        &self,
        db: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        let mut indexes = HashMap::new();
        for keykind in &[KeychainKind::External, KeychainKind::Internal] {
            indexes.insert(*keykind, db.get_last_index(*keykind)?.unwrap_or(0));
        }

        let mut known_txs: HashMap<_, _> = db
            .iter_txs(true)?
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect();
        let known_utxos: HashSet<_> = db.iter_utxos()?.into_iter().collect();

        //TODO list_since_blocks would be more efficient
        let current_utxo = self
            .client
            .list_unspent(Some(0), None, None, Some(true), None)?;
        debug!("current_utxo len {}", current_utxo.len());

        //TODO supported up to 1_000 txs, should use since_blocks or do paging
        let list_txs = self
            .client
            .list_transactions(None, Some(1_000), None, Some(true))?;
        let mut list_txs_ids = HashSet::new();

        for tx_result in list_txs.iter().filter(|t| {
            // list_txs returns all conflicting txs, we want to
            // filter out replaced tx => unconfirmed and not in the mempool
            t.info.confirmations > 0 || self.client.get_mempool_entry(&t.info.txid).is_ok()
        }) {
            let txid = tx_result.info.txid;
            list_txs_ids.insert(txid);
            if let Some(mut known_tx) = known_txs.get_mut(&txid) {
                let confirmation_time =
                    BlockTime::new(tx_result.info.blockheight, tx_result.info.blocktime);
                if confirmation_time != known_tx.confirmation_time {
                    // reorg may change tx height
                    debug!(
                        "updating tx({}) confirmation time to: {:?}",
                        txid, confirmation_time
                    );
                    known_tx.confirmation_time = confirmation_time;
                    db.set_tx(known_tx)?;
                }
            } else {
                //TODO check there is already the raw tx in db?
                let tx_result = self.client.get_transaction(&txid, Some(true))?;
                let tx: Transaction = deserialize(&tx_result.hex)?;
                let mut received = 0u64;
                let mut sent = 0u64;
                for output in tx.output.iter() {
                    if let Ok(Some((kind, index))) =
                        db.get_path_from_script_pubkey(&output.script_pubkey)
                    {
                        if index > *indexes.get(&kind).unwrap() {
                            indexes.insert(kind, index);
                        }
                        received += output.value;
                    }
                }

                for input in tx.input.iter() {
                    if let Some(previous_output) = db.get_previous_output(&input.previous_output)? {
                        if db.is_mine(&previous_output.script_pubkey)? {
                            sent += previous_output.value;
                        }
                    }
                }

                let td = TransactionDetails {
                    transaction: Some(tx),
                    txid: tx_result.info.txid,
                    confirmation_time: BlockTime::new(
                        tx_result.info.blockheight,
                        tx_result.info.blocktime,
                    ),
                    received,
                    sent,
                    fee: tx_result.fee.map(|f| f.as_sat().abs() as u64),
                };
                debug!(
                    "saving tx: {} tx_result.fee:{:?} td.fees:{:?}",
                    td.txid, tx_result.fee, td.fee
                );
                db.set_tx(&td)?;
            }
        }

        for known_txid in known_txs.keys() {
            if !list_txs_ids.contains(known_txid) {
                debug!("removing tx: {}", known_txid);
                db.del_tx(known_txid, false)?;
            }
        }

        // Filter out trasactions that are for script pubkeys that aren't in this wallet.
        let current_utxos = current_utxo
            .into_iter()
            .filter_map(
                |u| match db.get_path_from_script_pubkey(&u.script_pub_key) {
                    Err(e) => Some(Err(e)),
                    Ok(None) => None,
                    Ok(Some(path)) => Some(Ok(LocalUtxo {
                        outpoint: OutPoint::new(u.txid, u.vout),
                        keychain: path.0,
                        txout: TxOut {
                            value: u.amount.as_sat(),
                            script_pubkey: u.script_pub_key,
                        },
                        is_spent: false,
                    })),
                },
            )
            .collect::<Result<HashSet<_>, Error>>()?;

        let spent: HashSet<_> = known_utxos.difference(&current_utxos).collect();
        for utxo in spent {
            debug!("setting as spent utxo: {:?}", utxo);
            let mut spent_utxo = utxo.clone();
            spent_utxo.is_spent = true;
            db.set_utxo(&spent_utxo)?;
        }
        let received: HashSet<_> = current_utxos.difference(&known_utxos).collect();
        for utxo in received {
            debug!("adding utxo: {:?}", utxo);
            db.set_utxo(utxo)?;
        }

        for (keykind, index) in indexes {
            debug!("{:?} max {}", keykind, index);
            db.set_last_index(keykind, index)?;
        }

        Ok(())
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
        let rpc_version = client.version()?;

        let loaded_wallets = client.list_wallets()?;
        if loaded_wallets.contains(&wallet_name) {
            debug!("wallet already loaded {:?}", wallet_name);
        } else if list_wallet_dir(&client)?.contains(&wallet_name) {
            client.load_wallet(&wallet_name)?;
            debug!("wallet loaded {:?}", wallet_name);
        } else {
            // pre-0.21 use legacy wallets
            if rpc_version < 210_000 {
                client.create_wallet(&wallet_name, Some(true), None, None, None)?;
            } else {
                // TODO: move back to api call when https://github.com/rust-bitcoin/rust-bitcoincore-rpc/issues/225 is closed
                let args = [
                    Value::String(wallet_name.clone()),
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Null,
                    Value::Bool(false),
                    Value::Bool(true),
                ];
                let _: Value = client.call("createwallet", &args)?;
            }

            debug!("wallet created {:?}", wallet_name);
        }

        let is_descriptors = is_wallet_descriptor(&client)?;

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
            capabilities,
            is_descriptors,
            _storage_address: storage_address,
            skip_blocks: config.skip_blocks,
        })
    }
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

/// Returns whether a wallet is legacy or descriptors by calling `getwalletinfo`.
///
/// This API is mapped by bitcoincore_rpc, but it doesn't have the fields we need (either
/// "descriptors" or "format") so we have to call the RPC manually
fn is_wallet_descriptor(client: &Client) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct CallResult {
        descriptors: Option<bool>,
    }

    let result: CallResult = client.call("getwalletinfo", &[])?;
    Ok(result.descriptors.unwrap_or(false))
}

/// Factory of [`RpcBlockchain`] instances, implements [`BlockchainFactory`]
///
/// Internally caches the node url and authentication params and allows getting many different [`RpcBlockchain`]
/// objects for different wallet names and with different rescan heights.
///
/// ## Example
///
/// ```no_run
/// # use bdk::bitcoin::Network;
/// # use bdk::blockchain::BlockchainFactory;
/// # use bdk::blockchain::rpc::{Auth, RpcBlockchainFactory};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let factory = RpcBlockchainFactory {
///     url: "http://127.0.0.1:18332".to_string(),
///     auth: Auth::Cookie {
///         file: "/home/user/.bitcoin/.cookie".into(),
///     },
///     network: Network::Testnet,
///     wallet_name_prefix: Some("prefix-".to_string()),
///     default_skip_blocks: 100_000,
/// };
/// let main_wallet_blockchain = factory.build("main_wallet", Some(200_000))?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RpcBlockchainFactory {
    /// The bitcoin node url
    pub url: String,
    /// The bitcoin node authentication mechanism
    pub auth: Auth,
    /// The network we are using (it will be checked the bitcoin node network matches this)
    pub network: Network,
    /// The optional prefix used to build the full wallet name for blockchains
    pub wallet_name_prefix: Option<String>,
    /// Default number of blocks to skip which will be inherited by blockchain unless overridden
    pub default_skip_blocks: u32,
}

impl BlockchainFactory for RpcBlockchainFactory {
    type Inner = RpcBlockchain;

    fn build(
        &self,
        checksum: &str,
        override_skip_blocks: Option<u32>,
    ) -> Result<Self::Inner, Error> {
        RpcBlockchain::from_config(&RpcConfig {
            url: self.url.clone(),
            auth: self.auth.clone(),
            network: self.network,
            wallet_name: format!(
                "{}{}",
                self.wallet_name_prefix.as_ref().unwrap_or(&String::new()),
                checksum
            ),
            skip_blocks: Some(override_skip_blocks.unwrap_or(self.default_skip_blocks)),
        })
    }
}

#[cfg(test)]
#[cfg(any(feature = "test-rpc", feature = "test-rpc-legacy"))]
mod test {
    use super::*;
    use crate::testutils::blockchain_tests::TestClient;

    use bitcoin::Network;
    use bitcoincore_rpc::RpcApi;

    crate::bdk_blockchain_tests! {
        fn test_instance(test_client: &TestClient) -> RpcBlockchain {
            let config = RpcConfig {
                url: test_client.bitcoind.rpc_url(),
                auth: Auth::Cookie { file: test_client.bitcoind.params.cookie_file.clone() },
                network: Network::Regtest,
                wallet_name: format!("client-wallet-test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() ),
                skip_blocks: None,
            };
            RpcBlockchain::from_config(&config).unwrap()
        }
    }

    fn get_factory() -> (TestClient, RpcBlockchainFactory) {
        let test_client = TestClient::default();

        let factory = RpcBlockchainFactory {
            url: test_client.bitcoind.rpc_url(),
            auth: Auth::Cookie {
                file: test_client.bitcoind.params.cookie_file.clone(),
            },
            network: Network::Regtest,
            wallet_name_prefix: Some("prefix-".into()),
            default_skip_blocks: 0,
        };

        (test_client, factory)
    }

    #[test]
    fn test_rpc_blockchain_factory() {
        let (_test_client, factory) = get_factory();

        let a = factory.build("aaaaaa", None).unwrap();
        assert_eq!(a.skip_blocks, Some(0));
        assert_eq!(
            a.client
                .get_wallet_info()
                .expect("Node connection isn't working")
                .wallet_name,
            "prefix-aaaaaa"
        );

        let b = factory.build("bbbbbb", Some(100)).unwrap();
        assert_eq!(b.skip_blocks, Some(100));
        assert_eq!(
            b.client
                .get_wallet_info()
                .expect("Node connection isn't working")
                .wallet_name,
            "prefix-bbbbbb"
        );
    }
}
