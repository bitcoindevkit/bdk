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
use crate::descriptor::calc_checksum;
use crate::error::MissingCachedScripts;
use crate::{BlockTime, Error, FeeRate, KeychainKind, LocalUtxo, TransactionDetails};
use bitcoin::Script;
use bitcoincore_rpc::json::{
    GetTransactionResultDetailCategory, ImportMultiOptions, ImportMultiRequest,
    ImportMultiRequestScriptPubkey, ImportMultiRescanSince, ListTransactionResult,
    ListUnspentResultEntry, ScanningDetails,
};
use bitcoincore_rpc::jsonrpc::serde_json::{json, Value};
use bitcoincore_rpc::Auth as RpcAuth;
use bitcoincore_rpc::{Client, RpcApi};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
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

impl Deref for RpcBlockchain {
    type Target = Client;

    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

/// RpcBlockchain configuration options
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
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
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RpcSyncParams {
    /// The minimum number of scripts to scan for on initial sync.
    pub start_script_count: usize,
    /// Time in unix seconds in which initial sync will start scanning from (0 to start from genesis).
    pub start_time: u64,
    /// Forces every sync to use `start_time` as import timestamp.
    pub force_start_time: bool,
    /// RPC poll rate (in seconds) to get state updates.
    pub poll_rate_sec: u64,
}

impl Default for RpcSyncParams {
    fn default() -> Self {
        Self {
            start_script_count: 100,
            start_time: 0,
            force_start_time: false,
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
            .to_sat() as f64;

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
    fn wallet_setup<D>(&self, db: &RefCell<D>, prog: Box<dyn Progress>) -> Result<(), Error>
    where
        D: BatchDatabase,
    {
        let mut db = db.borrow_mut();
        let db = db.deref_mut();
        let batch = DbState::new(db, &self.sync_params, &*prog)?
            .sync_with_core(&self.client, self.is_descriptors)?
            .as_db_batch()?;

        db.commit_batch(batch)
    }
}

impl ConfigurableBlockchain for RpcBlockchain {
    type Config = RpcConfig;

    /// Returns RpcBlockchain backend creating an RPC client to a specific wallet named as the descriptor's checksum
    /// if it's the first time it creates the wallet in the node and upon return is granted the wallet is loaded
    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let wallet_url = format!("{}/wallet/{}", config.url, &config.wallet_name);

        let client = Client::new(wallet_url.as_str(), config.auth.clone().into())?;
        let rpc_version = client.version()?;

        info!("connected to '{}' with auth: {:?}", wallet_url, config.auth);

        if client.list_wallets()?.contains(&config.wallet_name) {
            info!("wallet already loaded: {}", config.wallet_name);
        } else if list_wallet_dir(&client)?.contains(&config.wallet_name) {
            client.load_wallet(&config.wallet_name)?;
            info!("wallet loaded: {}", config.wallet_name);
        } else {
            // pre-0.21 use legacy wallets
            if rpc_version < 210_000 {
                client.create_wallet(&config.wallet_name, Some(true), None, None, None)?;
            } else {
                // TODO: move back to api call when https://github.com/rust-bitcoin/rust-bitcoincore-rpc/issues/225 is closed
                let args = [
                    Value::String(config.wallet_name.clone()),
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Null,
                    Value::Bool(false),
                    Value::Bool(true),
                ];
                let _: Value = client.call("createwallet", &args)?;
            }

            info!("wallet created: {}", config.wallet_name);
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
struct DbState<'a, D> {
    db: &'a D,
    params: &'a RpcSyncParams,
    prog: &'a dyn Progress,

    ext_spks: Vec<Script>,
    int_spks: Vec<Script>,
    txs: HashMap<Txid, TransactionDetails>,
    utxos: HashSet<LocalUtxo>,
    last_indexes: HashMap<KeychainKind, u32>,

    // "deltas" to apply to database
    retained_txs: HashSet<Txid>, // txs to retain (everything else should be deleted)
    updated_txs: HashSet<Txid>,  // txs to update
    updated_utxos: HashSet<LocalUtxo>, // utxos to update
}

impl<'a, D: BatchDatabase> DbState<'a, D> {
    /// Obtain [DbState] from [crate::database::Database].
    fn new(db: &'a D, params: &'a RpcSyncParams, prog: &'a dyn Progress) -> Result<Self, Error> {
        let ext_spks = db.iter_script_pubkeys(Some(KeychainKind::External))?;
        let int_spks = db.iter_script_pubkeys(Some(KeychainKind::Internal))?;

        // This is a hack to see whether atleast one of the keychains comes from a derivable
        // descriptor. We assume that non-derivable descriptors always has a script count of 1.
        let last_count = std::cmp::max(ext_spks.len(), int_spks.len());
        let has_derivable = last_count > 1;

        // If at least one descriptor is derivable, we need to ensure scriptPubKeys are sufficiently
        // cached.
        if has_derivable && last_count < params.start_script_count {
            let inner_err = MissingCachedScripts {
                last_count,
                missing_count: params.start_script_count - last_count,
            };
            debug!("requesting more spks with: {:?}", inner_err);
            return Err(Error::MissingCachedScripts(inner_err));
        }

        let txs = db
            .iter_txs(true)?
            .into_iter()
            .map(|tx| (tx.txid, tx))
            .collect::<HashMap<_, _>>();

        let utxos = db.iter_utxos()?.into_iter().collect::<HashSet<_>>();

        let last_indexes = [KeychainKind::External, KeychainKind::Internal]
            .iter()
            .filter_map(|keychain| match db.get_last_index(*keychain) {
                Ok(li_opt) => li_opt.map(|li| Ok((*keychain, li))),
                Err(err) => Some(Err(err)),
            })
            .collect::<Result<HashMap<_, _>, Error>>()?;

        info!("initial db state: txs={} utxos={}", txs.len(), utxos.len());

        // "delta" fields
        let retained_txs = HashSet::with_capacity(txs.len());
        let updated_txs = HashSet::with_capacity(txs.len());
        let updated_utxos = HashSet::with_capacity(utxos.len());

        Ok(Self {
            db,
            params,
            prog,
            ext_spks,
            int_spks,
            txs,
            utxos,
            last_indexes,
            retained_txs,
            updated_txs,
            updated_utxos,
        })
    }

    /// Sync states of [BatchDatabase] and Core wallet.
    /// First we import all `scriptPubKey`s from database into core wallet
    fn sync_with_core(&mut self, client: &Client, is_descriptor: bool) -> Result<&mut Self, Error> {
        // this tells Core wallet where to sync from for imported scripts
        let start_epoch = if self.params.force_start_time {
            self.params.start_time
        } else {
            self.db
                .get_sync_time()?
                .map_or(self.params.start_time, |st| st.block_time.timestamp)
        };

        // sync scriptPubKeys from Database to Core wallet
        let scripts_iter = self.ext_spks.iter().chain(&self.int_spks);
        if is_descriptor {
            import_descriptors(client, start_epoch, scripts_iter)?;
        } else {
            import_multi(client, start_epoch, scripts_iter)?;
        }

        // wait for Core wallet to rescan (TODO: maybe make this async)
        await_wallet_scan(client, self.params.poll_rate_sec, self.prog)?;

        // obtain iterator of pagenated `listtransactions` RPC calls
        const LIST_TX_PAGE_SIZE: usize = 100; // item count per page
        let tx_iter = list_transactions(client, LIST_TX_PAGE_SIZE)?.filter(|item| {
            // filter out conflicting transactions - only accept transactions that are already
            // confirmed, or exists in mempool
            item.info.confirmations > 0 || client.get_mempool_entry(&item.info.txid).is_ok()
        });

        // iterate through chronological results of `listtransactions`
        for tx_res in tx_iter {
            let mut updated = false;

            let db_tx = self.txs.entry(tx_res.info.txid).or_insert_with(|| {
                updated = true;
                TransactionDetails {
                    txid: tx_res.info.txid,
                    transaction: None,

                    received: 0,
                    sent: 0,
                    fee: None,
                    confirmation_time: None,
                }
            });

            // update raw tx (if needed)
            let raw_tx =
                &*match &mut db_tx.transaction {
                    Some(raw_tx) => raw_tx,
                    db_tx_opt => {
                        updated = true;
                        db_tx_opt.insert(client.get_raw_transaction(
                            &tx_res.info.txid,
                            tx_res.info.blockhash.as_ref(),
                        )?)
                    }
                };

            // update fee (if needed)
            if let (None, Some(new_fee)) = (db_tx.fee, tx_res.detail.fee) {
                updated = true;
                db_tx.fee = Some(new_fee.to_sat().unsigned_abs());
            }

            // update confirmation time (if needed)
            let conf_time = BlockTime::new(tx_res.info.blockheight, tx_res.info.blocktime);
            if db_tx.confirmation_time != conf_time {
                updated = true;
                db_tx.confirmation_time = conf_time;
            }

            // update received (if needed)
            let received = Self::received_from_raw_tx(self.db, raw_tx)?;
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
                    self.db.get_path_from_script_pubkey(&txout.script_pubkey)?
                {
                    let utxo = LocalUtxo {
                        outpoint: OutPoint::new(tx_res.info.txid, tx_res.detail.vout),
                        txout,
                        keychain,
                        is_spent: false,
                    };
                    self.updated_utxos.insert(utxo);
                    self.update_last_index(keychain, index);
                }
            }

            // update tx deltas
            self.retained_txs.insert(tx_res.info.txid);
            if updated {
                self.updated_txs.insert(tx_res.info.txid);
            }
        }

        // obtain vector of `TransactionDetails::sent` changes
        let sent_updates = self
            .txs
            .values()
            // only bother to update txs that are retained
            .filter(|db_tx| self.retained_txs.contains(&db_tx.txid))
            // only bother to update txs where the raw tx is accessable
            .filter_map(|db_tx| (db_tx.transaction.as_ref().map(|tx| (tx, db_tx.sent))))
            // recalcuate sent value, only update txs in which sent value is changed
            .filter_map(|(raw_tx, old_sent)| {
                self.sent_from_raw_tx(raw_tx)
                    .map(|sent| {
                        if sent != old_sent {
                            Some((raw_tx.txid(), sent))
                        } else {
                            None
                        }
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;

        // record send updates
        sent_updates.iter().for_each(|&(txid, sent)| {
            // apply sent field changes
            self.txs.entry(txid).and_modify(|db_tx| db_tx.sent = sent);
            // mark tx as modified
            self.updated_txs.insert(txid);
        });

        // obtain UTXOs from Core wallet
        let core_utxos = client
            .list_unspent(Some(0), None, None, Some(true), None)?
            .into_iter()
            .filter_map(|utxo_entry| {
                let path_result = self
                    .db
                    .get_path_from_script_pubkey(&utxo_entry.script_pub_key)
                    .transpose()?;

                let utxo_result = match path_result {
                    Ok((keychain, index)) => {
                        self.update_last_index(keychain, index);
                        Ok(Self::make_local_utxo(utxo_entry, keychain, false))
                    }
                    Err(err) => Err(err),
                };

                Some(utxo_result)
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

    /// Calculates received amount from raw tx.
    fn received_from_raw_tx(db: &D, raw_tx: &Transaction) -> Result<u64, Error> {
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
    fn sent_from_raw_tx(&self, raw_tx: &Transaction) -> Result<u64, Error> {
        let get_output = |outpoint: &OutPoint| {
            let raw_tx = self.txs.get(&outpoint.txid)?.transaction.as_ref()?;
            raw_tx.output.get(outpoint.vout as usize)
        };

        raw_tx.input.iter().try_fold(0_u64, |sent, txin| {
            let v = match get_output(&txin.previous_output) {
                Some(prev_txo) => {
                    if self.db.is_mine(&prev_txo.script_pubkey)? {
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

    // updates the db state's last_index for the given keychain (if larger than current last_index)
    fn update_last_index(&mut self, keychain: KeychainKind, index: u32) {
        self.last_indexes
            .entry(keychain)
            .and_modify(|last| {
                if *last < index {
                    *last = index;
                }
            })
            .or_insert_with(|| index);
    }

    fn make_local_utxo(
        entry: ListUnspentResultEntry,
        keychain: KeychainKind,
        is_spent: bool,
    ) -> LocalUtxo {
        LocalUtxo {
            outpoint: OutPoint::new(entry.txid, entry.vout),
            txout: TxOut {
                value: entry.amount.to_sat(),
                script_pubkey: entry.script_pub_key,
            },
            keychain,
            is_spent,
        }
    }

    /// Prepare db batch operations.
    fn as_db_batch(&self) -> Result<D::Batch, Error> {
        let mut batch = self.db.begin_batch()?;
        let mut del_txs = 0_u32;

        // delete stale (not retained) txs from db
        self.txs
            .keys()
            .filter(|&txid| !self.retained_txs.contains(txid))
            .try_for_each(|txid| -> Result<(), Error> {
                batch.del_tx(txid, false)?;
                del_txs += 1;
                Ok(())
            })?;

        // update txs
        self.updated_txs
            .iter()
            .inspect(|&txid| debug!("updating tx: {}", txid))
            .try_for_each(|txid| batch.set_tx(self.txs.get(txid).unwrap()))?;

        // update utxos
        self.updated_utxos
            .iter()
            .inspect(|&utxo| debug!("updating utxo: {}", utxo.outpoint))
            .try_for_each(|utxo| batch.set_utxo(utxo))?;

        // update last indexes
        self.last_indexes
            .iter()
            .try_for_each(|(&keychain, &index)| batch.set_last_index(keychain, index))?;

        info!(
            "db batch updates: del_txs={}, update_txs={}, update_utxos={}",
            del_txs,
            self.updated_txs.len(),
            self.updated_utxos.len()
        );

        Ok(batch)
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

/// Calls the `listtransactions` RPC method in `page_size`s and returns iterator of the tx results
/// in chronological order.
///
/// `page_size` cannot be less than 1 and cannot be greater than 1000.
fn list_transactions(
    client: &Client,
    page_size: usize,
) -> Result<impl Iterator<Item = ListTransactionResult>, Error> {
    if !(1..=1000).contains(&page_size) {
        return Err(Error::Generic(format!(
            "Core RPC method `listtransactions` must have `page_size` in range [1 to 1000]: got {}",
            page_size
        )));
    }

    // `.take_while` helper to obtain the first error (TODO: remove when we can use `.map_while`)
    let mut got_err = false;

    // obtain results in batches (of `page_size`)
    let nested_list = (0_usize..)
        .map(|page_index| {
            client.list_transactions(
                None,
                Some(page_size),
                Some(page_size * page_index),
                Some(true),
            )
        })
        // take until returned rpc call is empty or until error
        // TODO: replace with the following when MSRV is 1.57.0:
        // `.map_while(|res| res.map(|l| if l.is_empty() { None } else { Some(l) }).transpose())`
        .take_while(|res| {
            if got_err || matches!(res, Ok(list) if list.is_empty()) {
                // break if last iteration was an error, or if the current result is empty
                false
            } else {
                // record whether result is error or not
                got_err = res.is_err();
                // continue on non-empty result or first error
                true
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::Rpc)?;

    // reverse here to have txs in chronological order
    Ok(nested_list.into_iter().rev().flatten())
}

fn await_wallet_scan(client: &Client, rate_sec: u64, progress: &dyn Progress) -> Result<(), Error> {
    #[derive(Deserialize)]
    struct CallResult {
        scanning: ScanningDetails,
    }

    let dur = Duration::from_secs(rate_sec);
    loop {
        match client.call::<CallResult>("getwalletinfo", &[])?.scanning {
            ScanningDetails::Scanning {
                duration,
                progress: pc,
            } => {
                debug!("scanning: duration={}, progress={}", duration, pc);
                progress.update(pc, Some(format!("elapsed for {} seconds", duration)))?;
                thread::sleep(dur);
            }
            ScanningDetails::NotScanning(_) => {
                progress.update(1.0, None)?;
                info!("scanning: done!");
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
    format!("{}#{}", desc, calc_checksum(&desc).unwrap())
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
    use crate::{
        descriptor::into_wallet_descriptor_checked, testutils::blockchain_tests::TestClient,
        wallet::utils::SecpCtx,
    };

    use bitcoin::{Address, Network};
    use bitcoincore_rpc::RpcApi;
    use log::LevelFilter;

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

    /// This test ensures that [list_transactions] always iterates through transactions in
    /// chronological order, independent of the `page_size`.
    #[test]
    fn test_list_transactions() {
        let _ = env_logger::builder()
            .filter_level(LevelFilter::Info)
            .default_format()
            .try_init();

        const DESC: &'static str = "wpkh(tpubD9zMNV59kgbWgKK55SHJugmKKSt6wQXczxpucGYqNKwGmJp1x7Ar2nrLUXYHDdCctXmyDoSCn2JVMzMUDfib3FaDhwxCEMUELoq19xLSx66/*)";
        const AMOUNT_PER_TX: u64 = 10_000;
        const TX_COUNT: u32 = 50;

        let secp = SecpCtx::default();
        let network = Network::Regtest;
        let (desc, ..) = into_wallet_descriptor_checked(DESC, &secp, network).unwrap();

        let (mut test_client, factory) = get_factory();
        let bc = factory.build("itertest", None).unwrap();

        // generate scripts (1 tx per script)
        let scripts = (0..TX_COUNT)
            .map(|index| desc.at_derivation_index(index).script_pubkey())
            .collect::<Vec<_>>();

        // import scripts and wait
        if bc.is_descriptors {
            import_descriptors(&bc.client, 0, scripts.iter()).unwrap();
        } else {
            import_multi(&bc.client, 0, scripts.iter()).unwrap();
        }
        await_wallet_scan(&bc.client, 2, &NoopProgress).unwrap();

        // create and broadcast txs
        let expected_txids = scripts
            .iter()
            .map(|script| {
                let addr = Address::from_script(script, network).unwrap();
                let txid =
                    test_client.receive(testutils! { @tx ( (@addr addr) => AMOUNT_PER_TX ) });
                test_client.generate(1, None);
                txid
            })
            .collect::<Vec<_>>();

        // iterate through different page sizes - should always return txs in chronological order
        [1000, 1, 2, 6, 25, 49, 50].iter().for_each(|page_size| {
            println!("trying with page_size: {}", page_size);

            let txids = list_transactions(&bc.client, *page_size)
                .unwrap()
                .map(|res| res.info.txid)
                .collect::<Vec<_>>();

            assert_eq!(txids.len(), expected_txids.len());
            assert_eq!(txids, expected_txids);
        });
    }
}
