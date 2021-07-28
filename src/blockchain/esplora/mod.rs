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

use serde::Deserialize;

use bitcoin::consensus;
use bitcoin::{BlockHash, Txid};

use crate::error::Error;
use crate::FeeRate;

#[cfg(all(
    feature = "esplora",
    feature = "reqwest",
    any(feature = "async-interface", target_arch = "wasm32"),
))]
mod reqwest;

#[cfg(all(
    feature = "esplora",
    feature = "reqwest",
    any(feature = "async-interface", target_arch = "wasm32"),
))]
pub use self::reqwest::*;

#[cfg(all(
    feature = "esplora",
    not(any(
        feature = "async-interface",
        feature = "reqwest",
        target_arch = "wasm32"
    )),
))]
mod ureq;

#[cfg(all(
    feature = "esplora",
    not(any(
        feature = "async-interface",
        feature = "reqwest",
        target_arch = "wasm32"
    )),
))]
pub use self::ureq::*;

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

/// Data type used when fetching transaction history from Esplora.
#[derive(Deserialize)]
pub struct EsploraGetHistory {
    txid: Txid,
    status: EsploraGetHistoryStatus,
}

#[derive(Deserialize)]
struct EsploraGetHistoryStatus {
    block_height: Option<usize>,
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

impl std::error::Error for EsploraError {}

#[cfg(feature = "ureq")]
impl_error!(::ureq::Error, Ureq, EsploraError);
#[cfg(feature = "ureq")]
impl_error!(::ureq::Transport, UreqTransport, EsploraError);
#[cfg(feature = "reqwest")]
impl_error!(::reqwest::Error, Reqwest, EsploraError);
impl_error!(io::Error, Io, EsploraError);
impl_error!(std::num::ParseIntError, Parsing, EsploraError);
impl_error!(consensus::encode::Error, BitcoinEncoding, EsploraError);
impl_error!(bitcoin::hashes::hex::Error, Hex, EsploraError);
