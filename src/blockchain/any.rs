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

//! Runtime-checked blockchain types
//!
//! This module provides the implementation of [`AnyBlockchain`] which allows switching the
//! inner [`Blockchain`] type at runtime.
//!
//! ## Example
//!
//! When paired with the use of [`ConfigurableBlockchain`], it allows creating any
//! blockchain type supported using a single line of code:
//!
//! ```no_run
//! # use bitcoin::Network;
//! # use bdk::blockchain::*;
//! # #[cfg(all(feature = "esplora", feature = "ureq"))]
//! # {
//! let config = serde_json::from_str("...")?;
//! let blockchain = AnyBlockchain::from_config(&config)?;
//! let height = blockchain.get_height();
//! # }
//! # Ok::<(), bdk::Error>(())
//! ```

use super::*;

macro_rules! impl_from {
    ( boxed $from:ty, $to:ty, $variant:ident, $( $cfg:tt )* ) => {
        $( $cfg )*
        impl From<$from> for $to {
            fn from(inner: $from) -> Self {
                <$to>::$variant(Box::new(inner))
            }
        }
    };
    ( $from:ty, $to:ty, $variant:ident, $( $cfg:tt )* ) => {
        $( $cfg )*
        impl From<$from> for $to {
            fn from(inner: $from) -> Self {
                <$to>::$variant(inner)
            }
        }
    };
}

macro_rules! impl_inner_method {
    ( $self:expr, $name:ident $(, $args:expr)* ) => {
        match $self {
            #[cfg(feature = "electrum")]
            AnyBlockchain::Electrum(inner) => inner.$name( $($args, )* ),
            #[cfg(feature = "esplora")]
            AnyBlockchain::Esplora(inner) => inner.$name( $($args, )* ),
            #[cfg(feature = "compact_filters")]
            AnyBlockchain::CompactFilters(inner) => inner.$name( $($args, )* ),
            #[cfg(feature = "rpc")]
            AnyBlockchain::Rpc(inner) => inner.$name( $($args, )* ),
        }
    }
}

/// Type that can contain any of the [`Blockchain`] types defined by the library
///
/// It allows switching backend at runtime
///
/// See [this module](crate::blockchain::any)'s documentation for a usage example.
pub enum AnyBlockchain {
    #[cfg(feature = "electrum")]
    #[cfg_attr(docsrs, doc(cfg(feature = "electrum")))]
    /// Electrum client
    Electrum(Box<electrum::ElectrumBlockchain>),
    #[cfg(feature = "esplora")]
    #[cfg_attr(docsrs, doc(cfg(feature = "esplora")))]
    /// Esplora client
    Esplora(Box<esplora::EsploraBlockchain>),
    #[cfg(feature = "compact_filters")]
    #[cfg_attr(docsrs, doc(cfg(feature = "compact_filters")))]
    /// Compact filters client
    CompactFilters(Box<compact_filters::CompactFiltersBlockchain>),
    #[cfg(feature = "rpc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rpc")))]
    /// RPC client
    Rpc(Box<rpc::RpcBlockchain>),
}

#[maybe_async]
impl Blockchain for AnyBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        maybe_await!(impl_inner_method!(self, get_capabilities))
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        maybe_await!(impl_inner_method!(self, broadcast, tx))
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        maybe_await!(impl_inner_method!(self, estimate_fee, target))
    }
}

#[maybe_async]
impl GetHeight for AnyBlockchain {
    fn get_height(&self) -> Result<u32, Error> {
        maybe_await!(impl_inner_method!(self, get_height))
    }
}

#[maybe_async]
impl GetTx for AnyBlockchain {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        maybe_await!(impl_inner_method!(self, get_tx, txid))
    }
}

#[maybe_async]
impl GetBlockHash for AnyBlockchain {
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error> {
        maybe_await!(impl_inner_method!(self, get_block_hash, height))
    }
}

#[maybe_async]
impl WalletSync for AnyBlockchain {
    fn wallet_sync<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        maybe_await!(impl_inner_method!(
            self,
            wallet_sync,
            database,
            progress_update
        ))
    }

    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        maybe_await!(impl_inner_method!(
            self,
            wallet_setup,
            database,
            progress_update
        ))
    }
}

impl_from!(boxed electrum::ElectrumBlockchain, AnyBlockchain, Electrum, #[cfg(feature = "electrum")]);
impl_from!(boxed esplora::EsploraBlockchain, AnyBlockchain, Esplora, #[cfg(feature = "esplora")]);
impl_from!(boxed compact_filters::CompactFiltersBlockchain, AnyBlockchain, CompactFilters, #[cfg(feature = "compact_filters")]);
impl_from!(boxed rpc::RpcBlockchain, AnyBlockchain, Rpc, #[cfg(feature = "rpc")]);

/// Type that can contain any of the blockchain configurations defined by the library
///
/// This allows storing a single configuration that can be loaded into an [`AnyBlockchain`]
/// instance. Wallets that plan to offer users the ability to switch blockchain backend at runtime
/// will find this particularly useful.
///
/// This type can be serialized from a JSON object like:
///
/// ```
/// # #[cfg(feature = "electrum")]
/// # {
/// use bdk::blockchain::{electrum::ElectrumBlockchainConfig, AnyBlockchainConfig};
/// let config: AnyBlockchainConfig = serde_json::from_str(
///     r#"{
///    "type" : "electrum",
///    "url" : "ssl://electrum.blockstream.info:50002",
///    "retry": 2,
///    "stop_gap": 20,
///    "validate_domain": true
/// }"#,
/// )
/// .unwrap();
/// assert_eq!(
///     config,
///     AnyBlockchainConfig::Electrum(ElectrumBlockchainConfig {
///         url: "ssl://electrum.blockstream.info:50002".into(),
///         retry: 2,
///         socks5: None,
///         timeout: None,
///         stop_gap: 20,
///         validate_domain: true,
///     })
/// );
/// # }
/// ```
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnyBlockchainConfig {
    #[cfg(feature = "electrum")]
    #[cfg_attr(docsrs, doc(cfg(feature = "electrum")))]
    /// Electrum client
    Electrum(electrum::ElectrumBlockchainConfig),
    #[cfg(feature = "esplora")]
    #[cfg_attr(docsrs, doc(cfg(feature = "esplora")))]
    /// Esplora client
    Esplora(esplora::EsploraBlockchainConfig),
    #[cfg(feature = "compact_filters")]
    #[cfg_attr(docsrs, doc(cfg(feature = "compact_filters")))]
    /// Compact filters client
    CompactFilters(compact_filters::CompactFiltersBlockchainConfig),
    #[cfg(feature = "rpc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "rpc")))]
    /// RPC client configuration
    Rpc(rpc::RpcConfig),
}

impl ConfigurableBlockchain for AnyBlockchain {
    type Config = AnyBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        Ok(match config {
            #[cfg(feature = "electrum")]
            AnyBlockchainConfig::Electrum(inner) => {
                AnyBlockchain::Electrum(Box::new(electrum::ElectrumBlockchain::from_config(inner)?))
            }
            #[cfg(feature = "esplora")]
            AnyBlockchainConfig::Esplora(inner) => {
                AnyBlockchain::Esplora(Box::new(esplora::EsploraBlockchain::from_config(inner)?))
            }
            #[cfg(feature = "compact_filters")]
            AnyBlockchainConfig::CompactFilters(inner) => AnyBlockchain::CompactFilters(Box::new(
                compact_filters::CompactFiltersBlockchain::from_config(inner)?,
            )),
            #[cfg(feature = "rpc")]
            AnyBlockchainConfig::Rpc(inner) => {
                AnyBlockchain::Rpc(Box::new(rpc::RpcBlockchain::from_config(inner)?))
            }
        })
    }
}

impl_from!(electrum::ElectrumBlockchainConfig, AnyBlockchainConfig, Electrum, #[cfg(feature = "electrum")]);
impl_from!(esplora::EsploraBlockchainConfig, AnyBlockchainConfig, Esplora, #[cfg(feature = "esplora")]);
impl_from!(compact_filters::CompactFiltersBlockchainConfig, AnyBlockchainConfig, CompactFilters, #[cfg(feature = "compact_filters")]);
impl_from!(rpc::RpcConfig, AnyBlockchainConfig, Rpc, #[cfg(feature = "rpc")]);
