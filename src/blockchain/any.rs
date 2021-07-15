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
//! In this example both `wallet_electrum` and `wallet_esplora` have the same type of
//! `Wallet<AnyBlockchain, MemoryDatabase>`. This means that they could both, for instance, be
//! assigned to a struct member.
//!
//! ```no_run
//! # use bitcoin::Network;
//! # use bdk::blockchain::*;
//! # use bdk::database::MemoryDatabase;
//! # use bdk::Wallet;
//! # #[cfg(feature = "electrum")]
//! # {
//! let electrum_blockchain = ElectrumBlockchain::from(electrum_client::Client::new("...")?);
//! let wallet_electrum: Wallet<AnyBlockchain, _> = Wallet::new(
//!     "...",
//!     None,
//!     Network::Testnet,
//!     MemoryDatabase::default(),
//!     electrum_blockchain.into(),
//! )?;
//! # }
//!
//! # #[cfg(feature = "esplora")]
//! # {
//! let esplora_blockchain = EsploraBlockchain::new("...", None, 20);
//! let wallet_esplora: Wallet<AnyBlockchain, _> = Wallet::new(
//!     "...",
//!     None,
//!     Network::Testnet,
//!     MemoryDatabase::default(),
//!     esplora_blockchain.into(),
//! )?;
//! # }
//!
//! # Ok::<(), bdk::Error>(())
//! ```
//!
//! When paired with the use of [`ConfigurableBlockchain`], it allows creating wallets with any
//! blockchain type supported using a single line of code:
//!
//! ```no_run
//! # use bitcoin::Network;
//! # use bdk::blockchain::*;
//! # use bdk::database::MemoryDatabase;
//! # use bdk::Wallet;
//! let config = serde_json::from_str("...")?;
//! let blockchain = AnyBlockchain::from_config(&config)?;
//! let wallet = Wallet::new(
//!     "...",
//!     None,
//!     Network::Testnet,
//!     MemoryDatabase::default(),
//!     blockchain,
//! )?;
//! # Ok::<(), bdk::Error>(())
//! ```

use super::*;

macro_rules! impl_from {
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
    Electrum(electrum::ElectrumBlockchain),
    #[cfg(feature = "esplora")]
    #[cfg_attr(docsrs, doc(cfg(feature = "esplora")))]
    /// Esplora client
    Esplora(esplora::EsploraBlockchain),
    #[cfg(feature = "compact_filters")]
    #[cfg_attr(docsrs, doc(cfg(feature = "compact_filters")))]
    /// Compact filters client
    CompactFilters(compact_filters::CompactFiltersBlockchain),
}

#[maybe_async]
impl Blockchain for AnyBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        maybe_await!(impl_inner_method!(self, get_capabilities))
    }

    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(impl_inner_method!(self, setup, database, progress_update))
    }
    fn sync<D: BatchDatabase, P: 'static + Progress>(
        &self,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(impl_inner_method!(self, sync, database, progress_update))
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        maybe_await!(impl_inner_method!(self, get_tx, txid))
    }
    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        maybe_await!(impl_inner_method!(self, broadcast, tx))
    }

    fn get_height(&self) -> Result<u32, Error> {
        maybe_await!(impl_inner_method!(self, get_height))
    }
    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        maybe_await!(impl_inner_method!(self, estimate_fee, target))
    }
}

impl_from!(electrum::ElectrumBlockchain, AnyBlockchain, Electrum, #[cfg(feature = "electrum")]);
impl_from!(esplora::EsploraBlockchain, AnyBlockchain, Esplora, #[cfg(feature = "esplora")]);
impl_from!(compact_filters::CompactFiltersBlockchain, AnyBlockchain, CompactFilters, #[cfg(feature = "compact_filters")]);

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
///    "stop_gap": 20
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
///     })
/// );
/// # }
/// ```
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone, PartialEq)]
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
}

impl ConfigurableBlockchain for AnyBlockchain {
    type Config = AnyBlockchainConfig;

    fn from_config(config: &Self::Config) -> Result<Self, Error> {
        Ok(match config {
            #[cfg(feature = "electrum")]
            AnyBlockchainConfig::Electrum(inner) => {
                AnyBlockchain::Electrum(electrum::ElectrumBlockchain::from_config(inner)?)
            }
            #[cfg(feature = "esplora")]
            AnyBlockchainConfig::Esplora(inner) => {
                AnyBlockchain::Esplora(esplora::EsploraBlockchain::from_config(inner)?)
            }
            #[cfg(feature = "compact_filters")]
            AnyBlockchainConfig::CompactFilters(inner) => AnyBlockchain::CompactFilters(
                compact_filters::CompactFiltersBlockchain::from_config(inner)?,
            ),
        })
    }
}

impl_from!(electrum::ElectrumBlockchainConfig, AnyBlockchainConfig, Electrum, #[cfg(feature = "electrum")]);
impl_from!(esplora::EsploraBlockchainConfig, AnyBlockchainConfig, Esplora, #[cfg(feature = "esplora")]);
impl_from!(compact_filters::CompactFiltersBlockchainConfig, AnyBlockchainConfig, CompactFilters, #[cfg(feature = "compact_filters")]);
