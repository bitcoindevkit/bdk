//! Blockchain
//!
//! This module provides traits that can be used to fetch [`Update`] data from the bitcoin
//! blockchain and perform other common functions needed by a [`Wallet`] user. There is an async
//! and blocking version of each trait.
//!
//! Also provides are implementations of these traits for the commonly used blockchain client protocols
//! [Electrum], [Esplora] and [BitcoinCoreRPC]. Creators of new or custom blockchain clients should
//! implement which ever of these traits that are applicable.
//!
//! [`Update`]: crate::wallet::Update
//! [Electrum]: TBD
//! [Esplora]: TBD
//! [BitcoinCoreRPC]: TBD

#[cfg(feature = "async")]
mod async_traits;

#[cfg(feature = "blocking")]
mod blocking_traits;

#[cfg(feature = "esplora_async")]
mod esplora_async;

// sync modes

/// Defines the options when syncing spks with an Electrum or Esplora blockchain client.
#[derive(Debug)]
pub struct SpkSyncMode {
    /// Sync all spks the wallet has ever derived
    pub all_spks: bool,
    /// Sync only derived spks that have not been used, only applies if `all_spks` is false
    pub unused_spks: bool,
    /// Sync wallet utxos
    pub utxos: bool,
    /// Sync unconfirmed transactions
    pub unconfirmed_tx: bool,
}

impl Default for SpkSyncMode {
    fn default() -> Self {
        Self {
            all_spks: false,
            unused_spks: true,
            utxos: true,
            unconfirmed_tx: true,
        }
    }
}

// trait errors

/// Errors that occur when using a blockchain client to get a transaction fee estimate.
#[derive(Debug)]
pub enum EstimateFeeError<C> {
    /// Insufficient data available to give an estimated [`FeeRate`] for the requested blocks
    InsufficientData,
    /// A blockchain client error
    ClientError(C),
}

/// Errors that can occur when using a blockchain client to scan script pub keys (spks).
#[derive(Debug)]
pub enum ScanSpksError<C> {
    /// A blockchain client error
    ClientError(C),
}
