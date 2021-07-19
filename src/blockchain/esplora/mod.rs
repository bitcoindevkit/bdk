//! Esplora
//!
//! This module defines a [`EsploraBlockchain`] struct that can query an Esplora
//! backend populate the wallet's [database](crate::database::Database) by:
//!
//! ## Example
//!
//! ```no_run
//! # use bdk::blockchain::esplora::EsploraBlockchain;
//! let blockchain = EsploraBlockchain::new("https://blockstream.info/testnet/api");
//! # Ok::<(), bdk::Error>(())
//! ```
//!
//! Esplora blockchain can use either `ureq` or `reqwest` for the HTTP client
//! depending on your needs (blocking or async respectively).
//!
//! Please note, to configure the Esplora HTTP client correctly use one of:
//! Blocking:  --features='esplora,ureq'
//! Async:     --features='async-interface,esplora,reqwest' --no-default-features
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
