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
//!     sync_params: None,
//! };
//! let blockchain = RpcBlockchain::from_config(&config);
//! ```

use crate::bitcoin::hashes::hex::ToHex;
use crate::bitcoin::{Network, OutPoint, Transaction, TxOut, Txid};
use crate::blockchain::*;
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::descriptor::get_checksum;
use crate::error::MissingCachedScripts;
use crate::{BlockTime, Error, FeeRate, KeychainKind, LocalUtxo, TransactionDetails};
use bitcoin::Script;
use bitcoincore_rpc::json::{
    GetTransactionResult, GetTransactionResultDetailCategory, ImportMultiOptions,
    ImportMultiRequest, ImportMultiRequestScriptPubkey, ImportMultiRescanSince,
    ListTransactionResult, ScanningDetails,
};
use bitcoincore_rpc::jsonrpc::serde_json::{json, Value};
use bitcoincore_rpc::Auth as RpcAuth;
use bitcoincore_rpc::{Client, RpcApi};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// The main struct for RPC backend implementing the [crate::blockchain::Blockchain] trait
#[derive(Debug)]
pub struct RpcBlockchain {
    /// Rpc client to the node, includes the wallet name
    client: Client,
    /// Whether the wallet is a "descriptor" or "legacy" wallet in Core
    is_descriptors: bool,
    /// Blockchain capabilities, cached here at startup
    capabilities: HashSet<Capability>,
    /// Sync parameters.
    sync_params: RpcSyncParams,
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
    /// Sync parameters
    pub sync_params: Option<RpcSyncParams>,
}

/// Sync parameters for Bitcoin Core RPC.
///
/// In general, BDK tries to sync `scriptPubKey`s cached in [`crate::database::Database`] with
/// `scriptPubKey`s imported in the Bitcoin Core Wallet. These parameters are used for determining
/// how the `importdescriptors` RPC calls are to be made.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RpcSyncParams {
    /// The minimum number of scripts to scan for on initial sync.
    pub start_script_count: usize,
    /// Time in unix seconds in which initial sync will start scanning from (0 to start from genesis).
    pub start_time: u64,
    /// RPC poll rate (in seconds) to get state updates.
    pub poll_rate_sec: u64,
}

impl Default for RpcSyncParams {
    fn default() -> Self {
        Self {
            start_script_count: 100,
            start_time: 0,
            poll_rate_sec: 3,
        }
    }
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

impl GetBlockHash for RpcBlockchain {
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error> {
        Ok(self.client.get_block_hash(height)?)
    }
}

impl WalletSync for RpcBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        db: &mut D,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        let db_scripts = db.iter_script_pubkeys(None)?;

        // this is a hack to check whether the scripts are coming from a derivable descriptor
        // we assume for non-derivable descriptors, the initial script count is always 1
        let is_derivable = db_scripts.len() > 1;

        // ensure db scripts meet start script count requirements
        if is_derivable && db_scripts.len() < self.sync_params.start_script_count {
            return Err(Error::MissingCachedScripts(MissingCachedScripts {
                last_count: db_scripts.len(),
                missing_count: self.sync_params.start_script_count - db_scripts.len(),
            }));
        }

        // this tells Core wallet where to sync from for imported scripts
        let start_epoch = db
            .get_sync_time()?
            .map_or(self.sync_params.start_time, |st| st.block_time.timestamp);

        // import all scripts from db into Core wallet
        if self.is_descriptors {
            import_descriptors(&self.client, start_epoch, db_scripts.iter())?;
        } else {
            import_multi(&self.client, start_epoch, db_scripts.iter())?;
        }

        // await sync (TODO: Maybe make this async)
        await_wallet_scan(
            &self.client,
            self.sync_params.poll_rate_sec,
            &*progress_update,
        )?;

        // begin db batch updates
        let mut db_batch = db.begin_batch();

        // update batch: obtain db state then update state with core txids
        DbState::from_db(db)?
            .update_state(&self.client, db)?
            .update_batch::<D>(&mut db_batch)?;

        // apply batch updates to db
        db.commit_batch(db_batch)
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

        Ok(RpcBlockchain {
            client,
            capabilities,
            is_descriptors,
            sync_params: config.sync_params.clone().unwrap_or_default(),
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

/// Represents the state of the [`crate::database::Database`].
struct DbState {
    txs: HashMap<Txid, TransactionDetails>,
    utxos: HashSet<LocalUtxo>,
    last_indexes: HashMap<KeychainKind, u32>,

    // "deltas" to apply to database
    retained_txs: HashSet<Txid>, // txs to retain (everything else should be deleted)
    updated_txs: HashSet<Txid>,  // txs to update
    updated_utxos: HashSet<LocalUtxo>, // utxos to update
    updated_last_indexes: HashSet<KeychainKind>,
}

impl DbState {
    /// Obtain [DbState] from [crate::database::Database].
    fn from_db<D: BatchDatabase>(db: &D) -> Result<Self, Error> {
        let txs = db
            .iter_txs(true)?
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect::<HashMap<_, _>>();
        let utxos = db.iter_utxos()?.into_iter().collect::<HashSet<_>>();
        let last_indexes = [KeychainKind::External, KeychainKind::Internal]
            .iter()
            .filter_map(|keychain| {
                db.get_last_index(*keychain)
                    .map(|v| v.map(|i| (*keychain, i)))
                    .transpose()
            })
            .collect::<Result<HashMap<_, _>, Error>>()?;

        let retained_txs = HashSet::with_capacity(txs.len());
        let updated_txs = HashSet::with_capacity(txs.len());
        let updated_utxos = HashSet::with_capacity(utxos.len());
        let updated_last_indexes = HashSet::with_capacity(last_indexes.len());

        Ok(Self {
            txs,
            utxos,
            last_indexes,
            retained_txs,
            updated_txs,
            updated_utxos,
            updated_last_indexes,
        })
    }

    /// Update [DbState] with Core wallet state
    fn update_state<D>(&mut self, client: &Client, db: &D) -> Result<&mut Self, Error>
    where
        D: BatchDatabase,
    {
        let tx_iter = CoreTxIter::new(client, 10);

        for tx_res in tx_iter {
            let tx_res = tx_res?;

            let mut updated = false;

            let db_tx = self.txs.entry(tx_res.info.txid).or_insert_with(|| {
                updated = true;
                TransactionDetails {
                    txid: tx_res.info.txid,
                    ..Default::default()
                }
            });

            // update raw tx (if needed)
            let raw_tx =
                match &db_tx.transaction {
                    Some(raw_tx) => raw_tx,
                    None => {
                        updated = true;
                        db_tx.transaction.insert(client.get_raw_transaction(
                            &tx_res.info.txid,
                            tx_res.info.blockhash.as_ref(),
                        )?)
                    }
                };

            // update fee (if needed)
            if let (None, Some(new_fee)) = (db_tx.fee, tx_res.detail.fee) {
                updated = true;
                db_tx.fee = Some(new_fee.as_sat().unsigned_abs());
            }

            // update confirmation time (if needed)
            let conf_time = BlockTime::new(tx_res.info.blockheight, tx_res.info.blocktime);
            if db_tx.confirmation_time != conf_time {
                updated = true;
                db_tx.confirmation_time = conf_time;
            }

            // update received (if needed)
            let received = Self::_received_from_raw_tx(db, raw_tx)?;
            if db_tx.received != received {
                updated = true;
                db_tx.received = received;
            }

            // check if tx has an immature coinbase output (add to updated UTXOs)
            // this is required because `listunspent` does not include immature coinbase outputs
            if tx_res.detail.category == GetTransactionResultDetailCategory::Immature {
                let txout = raw_tx
                    .output
                    .get(tx_res.detail.vout as usize)
                    .cloned()
                    .ok_or_else(|| {
                        Error::Generic(format!(
                            "Core RPC returned detail with invalid vout '{}' for tx '{}'",
                            tx_res.detail.vout, tx_res.info.txid,
                        ))
                    })?;

                if let Some((keychain, index)) =
                    db.get_path_from_script_pubkey(&txout.script_pubkey)?
                {
                    let utxo = LocalUtxo {
                        outpoint: OutPoint::new(tx_res.info.txid, tx_res.detail.vout),
                        txout,
                        keychain,
                        is_spent: false,
                    };
                    self.updated_utxos.insert(utxo);
                    self._update_last_index(keychain, index);
                }
            }

            // update tx deltas
            self.retained_txs.insert(tx_res.info.txid);
            if updated {
                self.updated_txs.insert(tx_res.info.txid);
            }
        }

        // update sent from tx inputs
        let sent_updates = self
            .txs
            .values()
            .filter_map(|db_tx| {
                let txid = self.retained_txs.get(&db_tx.txid)?;
                self._sent_from_raw_tx(db, db_tx.transaction.as_ref()?)
                    .map(|sent| {
                        if db_tx.sent != sent {
                            Some((*txid, sent))
                        } else {
                            None
                        }
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;

        // record send updates
        sent_updates.into_iter().for_each(|(txid, sent)| {
            self.txs.entry(txid).and_modify(|db_tx| db_tx.sent = sent);
            self.updated_txs.insert(txid);
        });

        // obtain UTXOs from Core wallet
        let core_utxos = client
            .list_unspent(Some(0), None, None, Some(true), None)?
            .into_iter()
            .filter_map(|utxo_res| {
                db.get_path_from_script_pubkey(&utxo_res.script_pub_key)
                    .transpose()
                    .map(|v| {
                        v.map(|(keychain, index)| {
                            // update last index if needed
                            self._update_last_index(keychain, index);

                            LocalUtxo {
                                outpoint: OutPoint::new(utxo_res.txid, utxo_res.vout),
                                keychain,
                                txout: TxOut {
                                    value: utxo_res.amount.as_sat(),
                                    script_pubkey: utxo_res.script_pub_key,
                                },
                                is_spent: false,
                            }
                        })
                    })
            })
            .collect::<Result<HashSet<_>, Error>>()?;

        // mark "spent utxos" to be updated in database
        let spent_utxos = self.utxos.difference(&core_utxos).cloned().map(|mut utxo| {
            utxo.is_spent = true;
            utxo
        });

        // mark new utxos to be added in database
        let new_utxos = core_utxos.difference(&self.utxos).cloned();

        // add to updated utxos
        self.updated_utxos.extend(spent_utxos.chain(new_utxos));

        Ok(self)
    }

    /// We want to filter out conflicting transactions.
    /// Only accept transactions that are already confirmed, or existing in mempool.
    fn _filter_tx(client: &Client, res: GetTransactionResult) -> Option<GetTransactionResult> {
        if res.info.confirmations > 0 || client.get_mempool_entry(&res.info.txid).is_ok() {
            Some(res)
        } else {
            debug!("tx filtered: {}", res.info.txid);
            None
        }
    }

    /// Calculates received amount from raw tx.
    fn _received_from_raw_tx<D: BatchDatabase>(db: &D, raw_tx: &Transaction) -> Result<u64, Error> {
        raw_tx.output.iter().try_fold(0_u64, |recv, txo| {
            let v = if db.is_mine(&txo.script_pubkey)? {
                txo.value
            } else {
                0
            };
            Ok(recv + v)
        })
    }

    /// Calculates sent from raw tx.
    fn _sent_from_raw_tx<D: BatchDatabase>(
        &self,
        db: &D,
        raw_tx: &Transaction,
    ) -> Result<u64, Error> {
        raw_tx.input.iter().try_fold(0_u64, |sent, txin| {
            let v = match self._previous_output(&txin.previous_output) {
                Some(prev_txo) => {
                    if db.is_mine(&prev_txo.script_pubkey)? {
                        prev_txo.value
                    } else {
                        0
                    }
                }
                None => 0_u64,
            };
            Ok(sent + v)
        })
    }

    fn _previous_output(&self, outpoint: &OutPoint) -> Option<&TxOut> {
        let prev_tx = self.txs.get(&outpoint.txid)?.transaction.as_ref()?;
        prev_tx.output.get(outpoint.vout as usize)
    }

    fn _update_last_index(&mut self, keychain: KeychainKind, index: u32) {
        let mut updated = false;

        self.last_indexes
            .entry(keychain)
            .and_modify(|last| {
                if *last < index {
                    updated = true;
                    *last = index;
                }
            })
            .or_insert_with(|| {
                updated = true;
                index
            });

        if updated {
            self.updated_last_indexes.insert(keychain);
        }
    }

    /// Prepare db batch operations.
    fn update_batch<D: BatchDatabase>(&self, batch: &mut D::Batch) -> Result<(), Error> {
        // delete stale txs from db
        // stale = not retained
        self.txs
            .keys()
            .filter(|&txid| !self.retained_txs.contains(txid))
            .try_for_each(|txid| batch.del_tx(txid, false).map(|_| ()))?;

        // update txs
        self.updated_txs
            .iter()
            .filter_map(|txid| self.txs.get(txid))
            .try_for_each(|txd| batch.set_tx(txd))?;

        // update utxos
        self.updated_utxos
            .iter()
            .inspect(|&utxo| println!("updating: {:?}", utxo.txout))
            .try_for_each(|utxo| batch.set_utxo(utxo))?;

        // update last indexes
        self.updated_last_indexes
            .iter()
            .map(|keychain| self.last_indexes.get_key_value(keychain).unwrap())
            .try_for_each(|(&keychain, &index)| batch.set_last_index(keychain, index))?;

        Ok(())
    }
}

fn import_descriptors<'a, S>(
    client: &Client,
    start_epoch: u64,
    scripts_iter: S,
) -> Result<(), Error>
where
    S: Iterator<Item = &'a Script>,
{
    let requests = Value::Array(
        scripts_iter
            .map(|script| {
                let desc = descriptor_from_script_pubkey(script);
                json!({ "timestamp": start_epoch, "desc": desc })
            })
            .collect(),
    );
    for v in client.call::<Vec<Value>>("importdescriptors", &[requests])? {
        match v["success"].as_bool() {
            Some(true) => continue,
            Some(false) => {
                return Err(Error::Generic(
                    v["error"]["message"]
                        .as_str()
                        .map_or("unknown error".into(), ToString::to_string),
                ))
            }
            _ => return Err(Error::Generic("Unexpected response form Core".to_string())),
        }
    }
    Ok(())
}

fn import_multi<'a, S>(client: &Client, start_epoch: u64, scripts_iter: S) -> Result<(), Error>
where
    S: Iterator<Item = &'a Script>,
{
    let requests = scripts_iter
        .map(|script| ImportMultiRequest {
            timestamp: ImportMultiRescanSince::Timestamp(start_epoch),
            script_pubkey: Some(ImportMultiRequestScriptPubkey::Script(script)),
            watchonly: Some(true),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let options = ImportMultiOptions { rescan: Some(true) };
    for v in client.import_multi(&requests, Some(&options))? {
        if let Some(err) = v.error {
            return Err(Error::Generic(format!(
                "{} (code: {})",
                err.message, err.code
            )));
        }
    }
    Ok(())
}

struct CoreTxIter<'a> {
    client: &'a Client,
    page_size: usize,
    page_index: usize,

    stack: Vec<ListTransactionResult>,
    done: bool,
}

impl<'a> CoreTxIter<'a> {
    fn new(client: &'a Client, page_size: usize) -> Self {
        Self {
            client,
            page_size,
            page_index: 0,
            stack: Vec::with_capacity(page_size),
            done: false,
        }
    }

    /// We want to filter out conflicting transactions.
    /// Only accept transactions that are already confirmed, or existing in mempool.
    fn tx_ok(&self, item: &ListTransactionResult) -> bool {
        item.info.confirmations > 0 || self.client.get_mempool_entry(&item.info.txid).is_ok()
    }
}

impl<'a> Iterator for CoreTxIter<'a> {
    type Item = Result<ListTransactionResult, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }

            if let Some(item) = self.stack.pop() {
                if self.tx_ok(&item) {
                    return Some(Ok(item));
                }
            }

            let res = self
                .client
                .list_transactions(
                    None,
                    Some(self.page_size),
                    Some(self.page_size * self.page_index),
                    Some(true),
                )
                .map_err(Error::Rpc);

            self.page_index += 1;

            let list = match res {
                Ok(list) => list,
                Err(err) => {
                    self.done = true;
                    return Some(Err(err));
                }
            };

            if list.is_empty() {
                self.done = true;
                return None;
            }

            self.stack = list;
        }
    }
}

fn get_scanning_details(client: &Client) -> Result<ScanningDetails, Error> {
    #[derive(Deserialize)]
    struct CallResult {
        scanning: ScanningDetails,
    }
    let result: CallResult = client.call("getwalletinfo", &[])?;
    Ok(result.scanning)
}

fn await_wallet_scan(
    client: &Client,
    poll_rate_sec: u64,
    progress_update: &dyn Progress,
) -> Result<(), Error> {
    let dur = Duration::from_secs(poll_rate_sec);
    loop {
        match get_scanning_details(client)? {
            ScanningDetails::Scanning { duration, progress } => {
                println!("scanning: duration={}, progress={}", duration, progress);
                progress_update
                    .update(progress, Some(format!("elapsed for {} seconds", duration)))?;
                thread::sleep(dur);
            }
            ScanningDetails::NotScanning(_) => {
                progress_update.update(1.0, None)?;
                println!("scanning: done!");
                return Ok(());
            }
        };
    }
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

fn descriptor_from_script_pubkey(script: &Script) -> String {
    let desc = format!("raw({})", script.to_hex());
    format!("{}#{}", desc, get_checksum(&desc).unwrap())
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
///     sync_params: None,
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
    /// Sync parameters
    pub sync_params: Option<RpcSyncParams>,
}

impl BlockchainFactory for RpcBlockchainFactory {
    type Inner = RpcBlockchain;

    fn build(
        &self,
        checksum: &str,
        _override_skip_blocks: Option<u32>,
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
            sync_params: self.sync_params.clone(),
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
                sync_params: None,
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
            sync_params: None,
        };

        (test_client, factory)
    }

    #[test]
    fn test_rpc_blockchain_factory() {
        let (_test_client, factory) = get_factory();

        let a = factory.build("aaaaaa", None).unwrap();
        assert_eq!(
            a.client
                .get_wallet_info()
                .expect("Node connection isn't working")
                .wallet_name,
            "prefix-aaaaaa"
        );

        let b = factory.build("bbbbbb", Some(100)).unwrap();
        assert_eq!(
            b.client
                .get_wallet_info()
                .expect("Node connection isn't working")
                .wallet_name,
            "prefix-bbbbbb"
        );
    }
}
