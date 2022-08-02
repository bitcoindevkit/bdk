//! Pruned Rpc Blockchain
//!
//! Backend that gets pruned blockchain data from Bitcoin Core RPC
//!
//! ## Example
//! ```no run
//! ```

use crate::bitcoin::hashes::hex::ToHex;
use crate::bitcoin::{Address, Network, Transaction, Txid};
use crate::database::{BatchDatabase, DatabaseUtils};
use crate::descriptor::get_checksum;
use crate::{blockchain::*, BlockTime};
use crate::{Error, FeeRate, KeychainKind, LocalUtxo, TransactionDetails};
use bitcoin::consensus::deserialize;
use bitcoin::{OutPoint, TxOut};
use bitcoincore_rpc::bitcoincore_rpc_json::Utxo;
use bitcoincore_rpc::json::{
    ImportMultiOptions, ImportMultiRequest, ImportMultiRequestScriptPubkey, ImportMultiRescanSince,
    ScanTxOutRequest, ScanTxOutResult,
};
use bitcoincore_rpc::jsonrpc::serde_json::{json, Value};
use bitcoincore_rpc::Auth as PrunedRpcAuth;
use bitcoincore_rpc::{Client, RpcApi};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

/// The main struct for Pruned RPC backend implementing the [crate::blockchain::Blockchain] trait
#[derive(Debug)]
pub struct PrunedRpcBlockchain {
    /// Rpc client to the node, includes the wallet name
    client: Client,
    /// Blockchain capabilities, cached here at startup
    capabilities: HashSet<Capability>,
    // Whether the wallet is a "descriptor" or "legacy" in core
    is_descriptor: bool,
    /// This is a fixed Address used as a hack key to store information on the node
    _storage_address: Address,
}

/// PrunedRpcBlockchain configuration option
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PrunedRpcConfig {
    /// The bitcoin node url
    pub url: String,
    /// The bitcoin node authentication mechanism
    pub auth: Auth,
    /// The network we are using (it will be checked the bitcoin node network matches this)
    pub network: Network,
    /// The wallet name in the bitcoin node, consider using
    /// [crate::wallet::wallet_name_from_descriptor] for this
    pub wallet_name: String,
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

impl From<Auth> for PrunedRpcAuth {
    fn from(auth: Auth) -> Self {
        match auth {
            Auth::None => PrunedRpcAuth::None,
            Auth::UserPass { username, password } => PrunedRpcAuth::UserPass(username, password),
            Auth::Cookie { file } => PrunedRpcAuth::CookieFile(file),
        }
    }
}

impl PrunedRpcBlockchain {}

impl Blockchain for PrunedRpcBlockchain {
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

impl GetTx for PrunedRpcBlockchain {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(Some(self.client.get_raw_transaction(txid, None)?))
    }
}

impl GetHeight for PrunedRpcBlockchain {
    fn get_height(&self) -> Result<u32, Error> {
        Ok(self.client.get_blockchain_info().map(|i| i.blocks as u32)?)
    }
}

impl GetBlockHash for PrunedRpcBlockchain {
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error> {
        Ok(self.client.get_block_hash(height)?)
    }
}

impl WalletSync for PrunedRpcBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &mut D,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        let mut script_pubkeys = database.iter_script_pubkeys(Some(KeychainKind::External))?;
        script_pubkeys.extend(database.iter_script_pubkeys(Some(KeychainKind::Internal))?);

        debug!(
            "importing {} script_pubkeys (some maybe aleady imported)",
            script_pubkeys.len()
        );

        if self.is_descriptor {
            let requests = Value::Array(
                script_pubkeys
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
            let requests: Vec<_> = script_pubkeys
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

        self.wallet_sync(database, progress_update)
    }

    fn wallet_sync<D: BatchDatabase>(
        &self,
        db: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        let mut script_pubkeys = db.iter_script_pubkeys(Some(KeychainKind::External))?;
        script_pubkeys.extend(db.iter_script_pubkeys(Some(KeychainKind::Internal))?);

        debug!(
            "importing {} script_pubkeys (some maybe aleady imported)",
            script_pubkeys.len()
        );

        // make a request using scantxout and loop through it here
        // the scan needs to have all the descriptors in it
        // see how to make sure you're scanning the minimum possible set
        let mut descriptors: Vec<ScanTxOutRequest> = vec![];
        for script in script_pubkeys {
            let desc = format!("raw({})", script.to_hex());
            descriptors.push(ScanTxOutRequest::Single(desc));
        }
        let res: ScanTxOutResult = self.client.scan_tx_out_set_blocking(&descriptors)?;
        // assume res is valid for now?
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

        // set current_utxo from res
        // if res.success == Some(false) {
        // idk what to do
        // Ok(())
        // }
        // if res.success == Some(true) {
        let mut current_utxo: Vec<Utxo> = Vec::new();
        for utxo in res.unspents {
            current_utxo.push(utxo);
        }
        let list_txs = self
            .client
            .list_transactions(None, Some(1_000), None, Some(true))?;
        let mut list_txs_ids = HashSet::new();

        for tx_result in list_txs.iter().filter(|t| {
            t.info.confirmations > 0 || self.client.get_mempool_entry(&t.info.txid).is_ok()
        }) {
            let txid = tx_result.info.txid;
            list_txs_ids.insert(txid);
            if let Some(mut known_tx) = known_txs.get_mut(&txid) {
                let confirmation_time =
                    BlockTime::new(tx_result.info.blockheight, tx_result.info.blocktime);
                if confirmation_time != known_tx.confirmation_time {
                    debug!(
                        "updating tx({}) confirmation time to: {:?}",
                        txid, confirmation_time
                    );
                    known_tx.confirmation_time = confirmation_time;
                    db.set_tx(known_tx)?;
                }
            } else {
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
                    fee: tx_result.fee.map(|f| f.as_sat().unsigned_abs()),
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
        // }
        Ok(())
    }
}

impl ConfigurableBlockchain for PrunedRpcBlockchain {
    type Config = PrunedRpcConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        let wallet_name = config.wallet_name.clone();
        let wallet_uri = format!("{}/wallet/{}", config.url, &wallet_name);
        debug!("connecting to {} auth:{:?}", wallet_uri, config.auth);

        let client = Client::new(wallet_uri.as_str(), config.auth.clone().into())?;
        let rpc_version = client.version()?;

        let loaded_wallets = client.list_wallets()?;
        if loaded_wallets.contains(&wallet_name) {
            debug!("wallet loaded {:?}", wallet_name);
        } else if list_wallet_dir(&client)?.contains(&wallet_name) {
            client.load_wallet(&wallet_name)?;
            debug!("wallet loaded {:?}", wallet_name);
        } else {
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

        // fixed address used only to store a label containing the synxed height in the node. Using
        // the same address as that used in RpcBlockchain
        let mut storage_address =
            Address::from_str("bc1qst0rewf0wm4kw6qn6kv0e5tc56nkf9yhcxlhqv").unwrap();
        storage_address.network = network;

        Ok(PrunedRpcBlockchain {
            client,
            capabilities,
            is_descriptor: is_descriptors,
            _storage_address: storage_address,
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

/// Returns whether a wallet is legacy or descriptor by calling `getwalletinfo`.
///
/// This API is mapped by bitcoincore_rpc, but it doesn't have the fields we need (either
/// "descriptor" or "format") so we have to call the RPC manually
fn is_wallet_descriptor(client: &Client) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct CallResult {
        descriptor: Option<bool>,
    }

    let result: CallResult = client.call("getwalletinfo", &[])?;
    Ok(result.descriptor.unwrap_or(false))
}

/// Factory of ['PrunedRpcBlockchain'] instance, implements ['BlockchainFactory']
///
/// Internally caches the node url and authentication params and allows getting many different
/// ['PrunedRpcBlockchain'] objects for different wallet names
///
/// ## Example
/// ```no_run
/// # use bdk::bitcoin::Network;
/// # use bdk::blockchain::BlockchainFactory;
/// # use bdk::blockchain::pruned_rpc::{Auth, PrunedRpcBlockchainFactory};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let factory = PrunedRpcBlockchainFactory {
///     url: "http://127.0.0.1:18332".to_string(),
///     auth: Auth::Cookie {
///         file: "/home/usr/.bitcoind/.cookie".into(),
///     },
///     network: Network::Testnet,
///     wallet_name_prefix: Some("prefix-".to_string()),
///     default_skip_blocks: 100_000,
/// };
/// let main_wallet_blockchain = factory.build("main_wallet", None)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct PrunedRpcBlockchainFactory {
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

impl BlockchainFactory for PrunedRpcBlockchainFactory {
    type Inner = PrunedRpcBlockchain;

    fn build(
        &self,
        checksum: &str,
        _override_skip_blocks: Option<u32>,
    ) -> Result<Self::Inner, Error> {
        PrunedRpcBlockchain::from_config(&PrunedRpcConfig {
            url: self.url.clone(),
            auth: self.auth.clone(),
            network: self.network,
            wallet_name: format!(
                "{}{}",
                self.wallet_name_prefix.as_ref().unwrap_or(&String::new()),
                checksum
            ),
        })
    }
}

#[cfg(test)]
#[cfg(any(feature = "test-pruned-rpc", feature = "test-pruned-rpc-legacy"))]
mod test {
    use super::*;
    use crate::testutils::blockchain_tests::TestClient;

    use bitcoin::Network;
    use bitcoincore_rpc::RpcApi;

    crate::bdk_blockchain_tests! {
        fn test_instance(test_client: &TestClient) -> PrunedRpcBlockchain {
            let config = PrunedRpcConfig {
                url: test_client.bitcoind.rpc_url(),
                auth: Auth::Cookie { file: test_client.bitcoind.params.cookie_file.clone() },
                network: Network::Regtest,
                wallet_name: format!("client-wallet-test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() ),
            };
            PrunedRpcBlockchain::from_config(&config).unwrap()
        }
    }

    fn get_factory() -> (TestClient, PrunedRpcBlockchainFactory) {
        let test_client = TestClient::default();

        let factory = PrunedRpcBlockchainFactory {
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
    fn test_pruned_rpc_factory() {
        let (_test_client, factory) = get_factory();

        let a = factory.build("aaaaaa", None).unwrap();
        // assert_eq!(a.skip_blocks, Some(0));
        assert_eq!(
            a.client
                .get_wallet_info()
                .expect("Node connection isn't working")
                .wallet_name,
            "prefix-aaaaaa"
        );

        let b = factory.build("bbbbbb", Some(100)).unwrap();
        // assert_eq!(b.skip_blocks, Some(100));
        assert_eq!(
            b.client
                .get_wallet_info()
                .expect("Node connection isn't working")
                .wallet_name,
            "prefix-bbbbbb"
        );
    }
}
