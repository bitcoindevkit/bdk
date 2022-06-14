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
use bitcoincore_rpc::Auth as RpcAuth;
use bitcoincore_rpc::{Client, RpcApi};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// The main struct for Pruned RPC backend implementing the [crate::blockchain::Blockchain] trait
#[derive(Debug)]
pub struct PrunedRpcBlockchain {
    /// Rpc client to the node, includes the wallet name
    client: Client,
    /// Whether the wallet is a "descriptor" or "legacy" wallet in Core
    _is_descriptors: bool,
    /// Blockchain capabilities, cached here at startup
    capabilities: HashSet<Capability>,
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

impl From<Auth> for RpcAuth {
    fn from(auth: Auth) -> Self {
        match auth {
            Auth::None => RpcAuth::None,
            Auth::UserPass { username, password } => RpcAuth::UserPass(username, password),
            Auth::Cookie { file } => RpcAuth::CookieFile(file),
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
        _database: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        todo!()
    }

    fn wallet_sync<D: BatchDatabase>(
        &self,
        _database: &mut D,
        _progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        todo!()
    }
}

impl ConfigurableBlockchain for PrunedRpcBlockchain {
    type Config = PrunedRpcConfig;

    fn from_config(_config: &Self::Config) -> Result<Self, Error> {
        todo!()
    }
}

/// Factory of ['PrunedRpcBlockchain'] instance, implements ['BlockchainFactory']
///
/// Internally caches the node url and authentication params and allows getting many different
/// ['PrunedRpcBlockchain'] objects for different wallet names
#[derive(Debug, Clone)]
pub struct PrunedRpcBlockchainFactory {}

impl BlockchainFactory for PrunedRpcBlockchainFactory {
    type Inner = PrunedRpcBlockchain;

    fn build(
        &self,
        _wallet_name: &str,
        _override_skip_blocks: Option<u32>,
    ) -> Result<Self::Inner, Error> {
        todo!()
    }
}

#[cfg(test)]
#[cfg(any(feature = "test-rpc", feature = "test-rpc-legacy"))]
mod test {}
