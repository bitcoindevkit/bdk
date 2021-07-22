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
//! # use bdk::blockchain::{RpcConfig, RpcBlockchain, ConfigurableBlockchain};
//! let config = RpcConfig {
//!     url: "127.0.0.1:18332".to_string(),
//!     auth: bitcoincore_rpc::Auth::CookieFile("/home/user/.bitcoin/.cookie".into()),
//!     network: bdk::bitcoin::Network::Testnet,
//!     wallet_name: "wallet_name".to_string(),
//!     skip_blocks: None,
//! };
//! let blockchain = RpcBlockchain::from_config(&config);
//! ```

use crate::bitcoin::consensus::deserialize;
use crate::bitcoin::{Address, Network, OutPoint, Transaction, TxOut, Txid};
use crate::blockchain::{Blockchain, Capability, ConfigurableBlockchain, Progress};
use crate::database::{BatchDatabase, DatabaseUtils};
use crate::descriptor::{get_checksum, IntoWalletDescriptor};
use crate::wallet::utils::SecpCtx;
use crate::{ConfirmationTime, Error, FeeRate, KeychainKind, LocalUtxo, TransactionDetails};
use bitcoincore_rpc::json::{
    GetAddressInfoResultLabel, ImportMultiOptions, ImportMultiRequest,
    ImportMultiRequestScriptPubkey, ImportMultiRescanSince,
};
use bitcoincore_rpc::jsonrpc::serde_json::Value;
use bitcoincore_rpc::{Auth, Client, RpcApi};
use log::debug;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

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

    /// This is a fixed Address used as a hack key to store information on the node
    _storage_address: Address,
}

/// RpcBlockchain configuration options
#[derive(Debug)]
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
                script_pubkey: Some(ImportMultiRequestScriptPubkey::Script(&s)),
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

        let current_height = self.get_height()?;

        // min because block invalidate may cause height to go down
        let node_synced = self.get_node_synced_height()?.min(current_height);

        //TODO call rescan in chunks (updating node_synced_height) so that in case of
        // interruption work can be partially recovered
        debug!(
            "rescan_blockchain from:{} to:{}",
            node_synced, current_height
        );
        self.client
            .rescan_blockchain(Some(node_synced as usize), Some(current_height as usize))?;
        progress_update.update(1.0, None)?;

        self.set_node_synced_height(current_height)?;

        self.sync(database, progress_update)
    }

    fn sync<D: BatchDatabase, P: 'static + Progress>(
        &self,
        db: &mut D,
        _progress_update: P,
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
            // list_txs returns all conflicting tx we want to
            // filter out replaced tx => unconfirmed and not in the mempool
            t.info.confirmations > 0 || self.client.get_mempool_entry(&t.info.txid).is_ok()
        }) {
            let txid = tx_result.info.txid;
            list_txs_ids.insert(txid);
            if let Some(mut known_tx) = known_txs.get_mut(&txid) {
                let confirmation_time =
                    ConfirmationTime::new(tx_result.info.blockheight, tx_result.info.blocktime);
                if confirmation_time != known_tx.confirmation_time {
                    // reorg may change tx height
                    debug!(
                        "updating tx({}) confirmation time to: {:?}",
                        txid, confirmation_time
                    );
                    known_tx.confirmation_time = confirmation_time;
                    db.set_tx(&known_tx)?;
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
                        sent += previous_output.value;
                    }
                }

                let td = TransactionDetails {
                    transaction: Some(tx),
                    txid: tx_result.info.txid,
                    confirmation_time: ConfirmationTime::new(
                        tx_result.info.blockheight,
                        tx_result.info.blocktime,
                    ),
                    received,
                    sent,
                    fee: tx_result.fee.map(|f| f.as_sat().abs() as u64),
                    verified: true,
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

        let current_utxos: HashSet<_> = current_utxo
            .into_iter()
            .map(|u| {
                Ok(LocalUtxo {
                    outpoint: OutPoint::new(u.txid, u.vout),
                    keychain: db
                        .get_path_from_script_pubkey(&u.script_pub_key)?
                        .ok_or(Error::TransactionNotFound)?
                        .0,
                    txout: TxOut {
                        value: u.amount.as_sat(),
                        script_pubkey: u.script_pub_key,
                    },
                })
            })
            .collect::<Result<_, Error>>()?;

        let spent: HashSet<_> = known_utxos.difference(&current_utxos).collect();
        for s in spent {
            debug!("removing utxo: {:?}", s);
            db.del_utxo(&s.outpoint)?;
        }
        let received: HashSet<_> = current_utxos.difference(&known_utxos).collect();
        for s in received {
            debug!("adding utxo: {:?}", s);
            db.set_utxo(s)?;
        }

        for (keykind, index) in indexes {
            debug!("{:?} max {}", keykind, index);
            db.set_last_index(keykind, index)?;
        }

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

        let client = Client::new(wallet_url, config.auth.clone())?;
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
        .into_wallet_descriptor(&secp, network)?
        .0
        .to_string();
    let mut wallet_name = get_checksum(&descriptor[..descriptor.find('#').unwrap()])?;
    if let Some(change_descriptor) = change_descriptor {
        let change_descriptor = change_descriptor
            .into_wallet_descriptor(&secp, network)?
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
            auth: Auth::CookieFile(test_client.bitcoind.params.cookie_file.clone()),
            network: Network::Regtest,
            wallet_name: format!("client-wallet-test-{:?}", std::time::SystemTime::now() ),
            skip_blocks: None,
        };
        RpcBlockchain::from_config(&config).unwrap()
    }
}
