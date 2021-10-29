//! Esplora
//!
//! This module defines a [`EsploraBlockchain`] struct that can query an Esplora
//! backend populate the wallet's [database](crate::database::Database) by:
//!
//! ## Example
//!
//! ```no_run
//! # use bdk::blockchain::esplora::EsploraBlockchain;
//! let blockchain = EsploraBlockchain::new("https://blockstream.info/testnet/api", 20);
//! # Ok::<(), bdk::Error>(())
//! ```
//!
//! Esplora blockchain can use either `ureq` or `reqwest` for the HTTP client
//! depending on your needs (blocking or async respectively).
//!
//! Please note, to configure the Esplora HTTP client correctly use one of:
//! Blocking:  --features='esplora,ureq'
//! Async:     --features='async-interface,esplora,reqwest' --no-default-features
use std::collections::HashMap;
use std::fmt;
use std::io;

use bitcoin::consensus;
use bitcoin::{BlockHash, Txid};

use crate::error::Error;
use crate::FeeRate;

#[cfg(feature = "reqwest")]
mod reqwest;

#[cfg(feature = "reqwest")]
pub use self::reqwest::*;

#[cfg(feature = "ureq")]
mod ureq;

#[cfg(feature = "ureq")]
pub use self::ureq::*;

mod api;

fn into_fee_rate(target: usize, estimates: HashMap<String, f64>) -> Result<FeeRate, Error> {
    let fee_val = estimates
        .into_iter()
        .map(|(k, v)| Ok::<_, std::num::ParseIntError>((k.parse::<usize>()?, v)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Error::Generic(e.to_string()))?
        .into_iter()
        .take_while(|(k, _)| k <= &target)
        .map(|(_, v)| v)
        .last()
        .unwrap_or(1.0);

    Ok(FeeRate::from_sat_per_vb(fee_val as f32))
}

/// Errors that can happen during a sync with [`EsploraBlockchain`]
#[derive(Debug)]
pub enum EsploraError {
    /// Error during ureq HTTP request
    #[cfg(feature = "ureq")]
    Ureq(::ureq::Error),
    /// Transport error during the ureq HTTP call
    #[cfg(feature = "ureq")]
    UreqTransport(::ureq::Transport),
    /// Error during reqwest HTTP request
    #[cfg(feature = "reqwest")]
    Reqwest(::reqwest::Error),
    /// HTTP response error
    HttpResponse(u16),
    /// IO error during ureq response read
    Io(io::Error),
    /// No header found in ureq response
    NoHeader,
    /// Invalid number returned
    Parsing(std::num::ParseIntError),
    /// Invalid Bitcoin data returned
    BitcoinEncoding(bitcoin::consensus::encode::Error),
    /// Invalid Hex data returned
    Hex(bitcoin::hashes::hex::Error),

    /// Transaction not found
    TransactionNotFound(Txid),
    /// Header height not found
    HeaderHeightNotFound(u32),
    /// Header hash not found
    HeaderHashNotFound(BlockHash),
}

impl fmt::Display for EsploraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Configuration for an [`EsploraBlockchain`]
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct EsploraBlockchainConfig {
    /// Base URL of the esplora service
    ///
    /// eg. `https://blockstream.info/api/`
    pub base_url: String,
    /// Optional URL of the proxy to use to make requests to the Esplora server
    ///
    /// The string should be formatted as: `<protocol>://<user>:<password>@host:<port>`.
    ///
    /// Note that the format of this value and the supported protocols change slightly between the
    /// sync version of esplora (using `ureq`) and the async version (using `reqwest`). For more
    /// details check with the documentation of the two crates. Both of them are compiled with
    /// the `socks` feature enabled.
    ///
    /// The proxy is ignored when targeting `wasm32`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Number of parallel requests sent to the esplora service (default: 4)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u8>,
    /// Stop searching addresses for transactions after finding an unused gap of this length.
    pub stop_gap: usize,
    /// Socket timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl EsploraBlockchainConfig {
    /// create a config with default values given the base url and stop gap
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            proxy: None,
            timeout: None,
            stop_gap: 20,
            concurrency: None,
        }
    }
}

impl std::error::Error for EsploraError {}

#[cfg(feature = "ureq")]
impl_error!(::ureq::Transport, UreqTransport, EsploraError);
#[cfg(feature = "reqwest")]
impl_error!(::reqwest::Error, Reqwest, EsploraError);
impl_error!(io::Error, Io, EsploraError);
impl_error!(std::num::ParseIntError, Parsing, EsploraError);
impl_error!(consensus::encode::Error, BitcoinEncoding, EsploraError);
impl_error!(bitcoin::hashes::hex::Error, Hex, EsploraError);

#[cfg(test)]
#[cfg(feature = "test-esplora")]
crate::bdk_blockchain_tests! {
    fn test_instance(test_client: &TestClient) -> EsploraBlockchain {
        EsploraBlockchain::new(&format!("http://{}",test_client.electrsd.esplora_url.as_ref().unwrap()), 20)
    }
}

const DEFAULT_CONCURRENT_REQUESTS: u8 = 4;
