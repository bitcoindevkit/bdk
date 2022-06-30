//! Pruned Rpc Blockchain
//!
//! Backend that gets pruned blockchain data from Bitcoin Core RPC
//!
//! ## Example
//! ```no run
//! ```

use crate::bitcoin::{Address, Network, Transaction, Txid};
use crate::blockchain::*;
use crate::database::BatchDatabase;
use crate::{Error, FeeRate};
use bitcoincore_rpc::jsonrpc::serde_json::Value;
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
    _is_descriptor: bool,
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

impl WalletSync for PrunedRpcBlockchain {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &mut D,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        todo!()
    }

    fn wallet_sync<D: BatchDatabase>(
        &self,
        db: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        todo!()
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
            _is_descriptor: is_descriptors,
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
#[cfg(any(feature = "test-rpc", feature = "test-rpc-legacy"))]
mod test {
    use super::*;
    use crate::testutils::blockchain_tests::TestClient;

    use bitcoin::Network;
    use bitcoincore_rpc::RpcApi;

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
